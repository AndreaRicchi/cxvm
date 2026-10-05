//! Editing JSON files that people also edit by hand: comments, order and layout stay as they
//! are, only the values cxvm sets change.

use std::path::Path;

use jsonc_parser::ParseOptions;
use jsonc_parser::cst::{CstArray, CstInputValue, CstNode, CstObject, CstRootNode};

use crate::error::{Error, Result};

/// An open JSON (or JSONC) document whose top level is an object.
pub(super) struct Document<'a> {
    path: &'a Path,
    /// How to make cxvm leave the file alone, for the hint of every error.
    off: &'a str,
    root: CstRootNode,
    pub object: CstObject,
}

impl<'a> Document<'a> {
    /// Parses `text`, the content of `path`; nothing but whitespace is an empty object.
    pub fn parse(path: &'a Path, text: &str, off: &'a str) -> Result<Self> {
        let invalid = |message: String| Error::InvalidFile {
            path: path.to_owned(),
            message,
            hint: format!("fix the file, or {off}"),
        };
        let root = CstRootNode::parse(text, &ParseOptions::default())
            .map_err(|err| invalid(err.to_string()))?;
        let object = match root.value() {
            Some(value) => value
                .as_object()
                .ok_or_else(|| invalid("its top level is not an object".into()))?,
            None => root.object_value_or_set(),
        };
        Ok(Self {
            path,
            off,
            root,
            object,
        })
    }

    /// An error about the content of the file.
    pub fn invalid(&self, message: String) -> Error {
        Error::InvalidFile {
            path: self.path.to_owned(),
            message,
            hint: format!("fix the file, or {}", self.off),
        }
    }

    /// The array `object.name`, created if the key is missing. Fails on any other value.
    pub fn array_in(&self, object: &CstObject, name: &str) -> Result<CstArray> {
        object
            .array_value_or_create(name)
            .ok_or_else(|| self.invalid(format!("`{name}` is not an array")))
    }

    /// The object `object.name`, created if the key is missing. Fails on any other value.
    pub fn object_in(&self, object: &CstObject, name: &str) -> Result<CstObject> {
        object
            .object_value_or_create(name)
            .ok_or_else(|| self.invalid(format!("`{name}` is not an object")))
    }

    /// The document as text, ending in a newline.
    pub fn finish(self) -> String {
        let mut text = self.root.to_string();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text
    }
}

/// The string `node` holds, if it is a string.
pub(super) fn string(node: &CstNode) -> Option<String> {
    node.as_string_lit()?.decoded_value().ok()
}

/// The string value of `object.name`, if there is one.
pub(super) fn string_in(object: &CstObject, name: &str) -> Option<String> {
    string(&object.get(name)?.value()?)
}

/// Sets `object.name` to the string `value`, in place if the key exists.
pub(super) fn set_string(object: &CstObject, name: &str, value: &str) {
    let value = CstInputValue::String(value.to_owned());
    match object.get(name) {
        Some(prop) => prop.set_value(value),
        None => {
            object.append(name, value);
        }
    }
}
