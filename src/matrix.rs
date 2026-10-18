use serde::Deserialize;
use std::collections::HashMap;
use tracing::error;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct MatrixDefinition {
    /// Boolean expression for this matrix
    trigger: String,
}

impl MatrixDefinition {
    pub fn eval(&self, sources: &HashMap<String, bool>) -> bool {
        error!("todo");
        false
    }
}
