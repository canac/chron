use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawConfig {
    pub shell: Option<String>,
    #[serde(rename = "onError")]
    pub on_error: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Config {
    pub shell: String,
    pub on_error: Option<String>,
}
