mod function_definition_macro;
mod function_traits;
pub(super) mod functions;
mod http_fetch_request;

use sqlparser::ast::FunctionArg;

use crate::webserver::http_request_info::RequestInfo;

use super::syntax_tree::SqlPageFunctionCall;
use super::syntax_tree::StmtParam;

use super::sql::FormatArguments;
use anyhow::Context;

pub(super) fn func_call_to_param(func_name: &str, arguments: &mut [FunctionArg]) -> StmtParam {
    SqlPageFunctionCall::from_func_call(func_name, arguments)
        .with_context(|| {
            format!(
                "Invalid function call: sqlpage.{func_name}({})",
                FormatArguments(arguments)
            )
        })
        .map_or_else(
            |e| StmtParam::Error(format!("{e:#}")),
            StmtParam::FunctionCall,
        )
}
