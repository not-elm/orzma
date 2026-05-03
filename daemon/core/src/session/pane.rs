use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct PaneStore(HashMap<PaneId, Pane>);

#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PaneId(pub String);

#[derive(Clone, Debug)]
pub struct Pane {}

#[derive(Clone, Debug)]
pub struct PaneLayout {}
