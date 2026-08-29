use std::string::String;
use std::vec::Vec;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandTarget {
    pub program: String,
    pub args: Vec<String>,
}
