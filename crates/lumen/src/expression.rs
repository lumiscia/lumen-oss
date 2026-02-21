use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ParsedExpr {
    pub source: String,
}

#[derive(Debug, Error)]
pub enum ExprParseError {
    #[error("expression parsing not implemented yet")]
    NotImplemented,
}

#[derive(Debug, Error)]
pub enum ExprEvalError {
    #[error("expression evaluation not implemented yet")]
    NotImplemented,
}

pub fn parse_expr(source: &str) -> Result<ParsedExpr, ExprParseError> {
    if source.trim().is_empty() {
        return Err(ExprParseError::NotImplemented);
    }
    Ok(ParsedExpr {
        source: source.to_string(),
    })
}
