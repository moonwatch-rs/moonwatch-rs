use schemars::JsonSchema;
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PipelineConfig {

}

impl PipelineConfig {
    pub fn new() -> PipelineConfig {
        PipelineConfig {

        }
    }
}
