use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct PackageJson {
    pub name: String,
    pub main: String,
}
