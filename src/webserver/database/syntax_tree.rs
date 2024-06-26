/// This module contains the syntax tree for sqlpage statement parameters.
/// In a query like `SELECT sqlpage.some_function($my_param)`,
/// The stored database statement will be just `SELECT $1`,
/// and the `StmtParam` will contain a the following tree:
///
/// ```text
/// StmtParam::FunctionCall(
///    SqlPageFunctionCall {
///       function: SqlPageFunctionName::some_function,
///      arguments: vec![StmtParam::Get("$my_param")]
///   }
/// )
/// ```
use std::borrow::Cow;
use std::str::FromStr;

use sqlparser::ast::FunctionArg;

use crate::webserver::database::sql::function_arg_to_stmt_param;
use crate::webserver::http::SingleOrVec;
use crate::webserver::http_request_info::RequestInfo;

use super::sqlpage_functions::functions::SqlPageFunctionName;
use anyhow::{anyhow, Context as _};

/// Represents a parameter to a SQL statement.
/// Objects of this type are created during SQL parsing.
/// Every time a SQL statement is executed, the parameters are evaluated to produce the actual values that are passed to the database.
/// Parameter evaluation can involve asynchronous operations, and extracting values from the request.
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum StmtParam {
    Get(String),
    Post(String),
    GetOrPost(String),
    Error(String),
    Literal(String),
    Null,
    Concat(Vec<StmtParam>),
    FunctionCall(SqlPageFunctionCall),
}

impl std::fmt::Display for StmtParam {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StmtParam::Get(name) => write!(f, "?{name}"),
            StmtParam::Post(name) => write!(f, ":{name}"),
            StmtParam::GetOrPost(name) => write!(f, "${name}"),
            StmtParam::Literal(x) => write!(f, "'{}'", x.replace('\'', "''")),
            StmtParam::Null => write!(f, "NULL"),
            StmtParam::Concat(items) => {
                write!(f, "CONCAT(")?;
                for item in items {
                    write!(f, "{item}, ")?;
                }
                write!(f, ")")
            }
            StmtParam::FunctionCall(call) => write!(f, "{call}"),
            StmtParam::Error(x) => {
                if let Some((i, _)) = x.char_indices().nth(21) {
                    write!(f, "## {}... ##", &x[..i])
                } else {
                    write!(f, "## {x} ##")
                }
            }
        }
    }
}

/// Represents a call to a `sqlpage.` function.
/// Objects of this type are created during SQL parsing and used to evaluate the function at runtime.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SqlPageFunctionCall {
    pub function: SqlPageFunctionName,
    pub arguments: Vec<StmtParam>,
}

impl SqlPageFunctionCall {
    pub fn from_func_call(func_name: &str, arguments: &mut [FunctionArg]) -> anyhow::Result<Self> {
        let function = SqlPageFunctionName::from_str(func_name)?;
        let arguments = arguments
            .iter_mut()
            .map(|arg| {
                function_arg_to_stmt_param(arg)
                    .ok_or_else(|| anyhow!("Passing \"{arg}\" as a function argument is not supported.\n\n\
                    The only supported sqlpage function argument types are : \n\
                      - variables (such as $my_variable), \n\
                      - other sqlpage function calls (such as sqlpage.cookie('my_cookie')), \n\
                      - literal strings (such as 'my_string'), \n\
                      - concatenations of the above (such as CONCAT(x, y)).\n\n\
                    Arbitrary SQL expressions as function arguments are not supported.\n\
                    Try executing the SQL expression in a separate SET expression, then passing it to the function:\n\n\
                    SET $my_parameter = {arg}; \n\
                    SELECT ... {function}(... $my_parameter ...) ...
                    "))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Self {
            function,
            arguments,
        })
    }

    pub async fn evaluate<'a>(
        &self,
        request: &'a RequestInfo,
    ) -> anyhow::Result<Option<Cow<'a, str>>> {
        let evaluated_args = self.arguments.iter().map(|x| extract_req_param(x, request));
        let evaluated_args = futures_util::future::try_join_all(evaluated_args).await?;
        self.function.evaluate(request, evaluated_args).await
    }
}

impl std::fmt::Display for SqlPageFunctionCall {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}(", self.function)?;
        // interleave the arguments with commas
        let mut it = self.arguments.iter();
        if let Some(x) = it.next() {
            write!(f, "{x}")?;
        }
        for x in it {
            write!(f, ", {x}")?;
        }
        write!(f, ")")
    }
}

/// Extracts the value of a parameter from the request.
/// Returns `Ok(None)` when NULL should be used as the parameter value.
pub(super) async fn extract_req_param<'a>(
    param: &StmtParam,
    request: &'a RequestInfo,
) -> anyhow::Result<Option<Cow<'a, str>>> {
    Ok(match param {
        // sync functions
        StmtParam::Get(x) => request.get_variables.get(x).map(SingleOrVec::as_json_str),
        StmtParam::Post(x) => request.post_variables.get(x).map(SingleOrVec::as_json_str),
        StmtParam::GetOrPost(x) => request
            .post_variables
            .get(x)
            .or_else(|| request.get_variables.get(x))
            .map(SingleOrVec::as_json_str),
        StmtParam::Error(x) => anyhow::bail!("{}", x),
        StmtParam::Literal(x) => Some(Cow::Owned(x.to_string())),
        StmtParam::Null => None,
        StmtParam::Concat(args) => concat_params(&args[..], request).await?,
        StmtParam::FunctionCall(func) => func.evaluate(request).await.with_context(|| {
            format!(
                "Error in function call {func}.\nExpected {:#}",
                func.function
            )
        })?,
    })
}

async fn concat_params<'a>(
    args: &[StmtParam],
    request: &'a RequestInfo,
) -> anyhow::Result<Option<Cow<'a, str>>> {
    let mut result = String::new();
    for arg in args {
        let Some(arg) = Box::pin(extract_req_param(arg, request)).await? else {
            return Ok(None);
        };
        result.push_str(&arg);
    }
    Ok(Some(Cow::Owned(result)))
}
