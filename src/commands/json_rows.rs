//! Reshaping a local JSON file into the newline-delimited JSON a load reads.
//!
//! A `json` load reads one JSON value per line. The JSON people actually have
//! on disk is often something else — an array of objects (what an HTTP API
//! returns, what `jq` writes), or a single pretty-printed document — and the
//! server refuses both with a parser message about the bytes ("failed to infer
//! JSON schema: EOF while parsing a list"), not about the shape. So the load
//! rewrites those shapes to newline-delimited JSON before uploading, and
//! `--file data.json` works whatever is inside it.
//!
//! Newline-delimited input is uploaded untouched: [`shape_of`] reads the first
//! record, not the file, so a large `.jsonl` is never rewritten to say the same
//! thing.

use std::io::{Read, Write};

use serde::de::{DeserializeSeed, Deserializer, SeqAccess, Visitor};
use serde_json::Value;

/// How many leading bytes [`shape_of`] wants to decide.
///
/// Large enough that the first record of any plausible newline-delimited file
/// is whole within it. A first record longer than this reads as
/// [`Shape::Values`] and is rewritten — a slower path to the same rows, never a
/// wrong one, since a stream of values is exactly what newline-delimited JSON
/// is.
pub const SNIFF_BYTES: usize = 64 * 1024;

/// The JSON shape a source carries, as read off its first bytes.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Shape {
    /// One whole JSON value per line — what a `json` load already reads.
    Ndjson,
    /// A single JSON array whose elements are the rows.
    Array,
    /// Whitespace-separated JSON values: one pretty-printed document, several
    /// concatenated, or records spanning lines.
    Values,
}

impl Shape {
    /// Whether a load needs this shape rewritten before upload.
    pub fn needs_rewrite(self) -> bool {
        self != Shape::Ndjson
    }
}

/// The shape of a JSON source, read from `head` — its first [`SNIFF_BYTES`]
/// bytes, or the whole thing when it is shorter.
///
/// Decided on two questions, in order: does it open an array, and is its first
/// line a whole value on its own? Only the second is a fast path worth
/// protecting, so an inconclusive answer errs towards rewriting.
pub fn shape_of(head: &[u8]) -> Shape {
    // Lossy is right for a sniff: `head` can end mid-character when it is a
    // truncated read, and a replacement char there changes no answer below.
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start();
    if text.starts_with('[') {
        return Shape::Array;
    }
    let first_line = text.split('\n').next().unwrap_or_default();
    if serde_json::from_str::<Value>(first_line).is_ok() {
        Shape::Ndjson
    } else {
        Shape::Values
    }
}

/// Read every row `src` carries in `shape` and write it to `out` as one
/// compact JSON object per line. Returns the row count.
///
/// Streams: rows are read and written one at a time, so a multi-gigabyte array
/// costs one row of memory rather than the whole document.
pub fn to_ndjson<R: Read, W: Write>(shape: Shape, src: R, out: W) -> Result<u64, String> {
    let mut rows = Rows { out, count: 0 };
    let mut de = serde_json::Deserializer::from_reader(src);
    match shape {
        // The elements of one array, read through the array rather than into it.
        Shape::Array => {
            (&mut rows).deserialize(&mut de).map_err(describe)?;
            // Reject anything after the array's close: a file holding `[…] [b`
            // is a truncated write, not two tables.
            de.end().map_err(describe)?;
        }
        // A stream of whole values, which is what both other shapes are.
        Shape::Ndjson | Shape::Values => {
            for value in de.into_iter::<Value>() {
                rows.write(value.map_err(describe)?)?;
            }
        }
    }
    rows.out.flush().map_err(|e| format!("{e}"))?;
    Ok(rows.count)
}

/// The rows written so far, and where they go.
struct Rows<W: Write> {
    out: W,
    count: u64,
}

impl<W: Write> Rows<W> {
    /// Write one row, refusing a value that is not an object: a load needs
    /// named columns, and a bare scalar or list names none.
    fn write(&mut self, value: Value) -> Result<(), String> {
        if !value.is_object() {
            return Err(format!(
                "a json load reads one object per row, and this file carries {} \
                 among its rows — reshape it so every row is an object",
                type_of(&value)
            ));
        }
        serde_json::to_writer(&mut self.out, &value).map_err(|e| format!("{e}"))?;
        self.out.write_all(b"\n").map_err(|e| format!("{e}"))?;
        self.count += 1;
        Ok(())
    }
}

// Reads the elements of a top-level array without holding the array: serde
// hands each element over as it is parsed, and each is written and dropped
// before the next is read.
impl<'de, W: Write> DeserializeSeed<'de> for &mut Rows<W> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, de: D) -> Result<(), D::Error> {
        de.deserialize_seq(self)
    }
}

impl<'de, W: Write> Visitor<'de> for &mut Rows<W> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "a json array of objects")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(value) = seq.next_element::<Value>()? {
            // Our own message, carried out through serde so the row's line and
            // column travel with it.
            self.write(value).map_err(serde::de::Error::custom)?;
        }
        Ok(())
    }
}

/// The JSON type name of `value`, for the not-an-object message.
fn type_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "a null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

/// A parse failure, as the CLI reports it. `serde_json` already appends the
/// line and column, including for the messages [`Rows::write`] raises.
fn describe(err: serde_json::Error) -> String {
    format!("{err}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ndjson(src: &str) -> Result<(u64, String), String> {
        let shape = shape_of(src.as_bytes());
        let mut out: Vec<u8> = Vec::new();
        let rows = to_ndjson(shape, src.as_bytes(), &mut out)?;
        Ok((rows, String::from_utf8(out).unwrap()))
    }

    #[test]
    fn newline_delimited_json_is_left_alone() {
        assert_eq!(
            shape_of(b"{\"id\":1}\n{\"id\":2}\n"),
            Shape::Ndjson,
            "a first line that is a whole value means the file is already ndjson"
        );
        // A single-line record with no trailing newline is still ndjson.
        assert_eq!(shape_of(b"{\"id\":1}"), Shape::Ndjson);
    }

    #[test]
    fn an_array_of_objects_becomes_one_row_per_line() {
        let (rows, out) =
            ndjson("[\n  {\"id\":1,\"n\":\"a\"},\n  {\"id\":2,\"n\":\"b\"}\n]\n").unwrap();
        assert_eq!(rows, 2);
        assert_eq!(out, "{\"id\":1,\"n\":\"a\"}\n{\"id\":2,\"n\":\"b\"}\n");
    }

    #[test]
    fn a_pretty_printed_object_becomes_one_row() {
        let (rows, out) = ndjson("{\n  \"id\": 1,\n  \"n\": \"a\"\n}\n").unwrap();
        assert_eq!(rows, 1);
        assert_eq!(out, "{\"id\":1,\"n\":\"a\"}\n");
    }

    #[test]
    fn concatenated_objects_become_one_row_each() {
        let (rows, out) = ndjson("{\"id\":1} {\"id\":2}{\"id\":3}").unwrap();
        assert_eq!(rows, 3);
        assert_eq!(out, "{\"id\":1}\n{\"id\":2}\n{\"id\":3}\n");
    }

    #[test]
    fn a_row_that_is_not_an_object_is_refused() {
        let err = ndjson("[{\"id\":1}, 7]").unwrap_err();
        assert!(
            err.contains("object"),
            "the error should say a row must be an object, got: {err}"
        );
    }

    #[test]
    fn malformed_json_is_refused() {
        assert!(ndjson("[{\"id\":1},").is_err());
    }

    #[test]
    fn an_empty_source_yields_no_rows() {
        assert_eq!(ndjson("   \n").unwrap().0, 0);
    }
}
