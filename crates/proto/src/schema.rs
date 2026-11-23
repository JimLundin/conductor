//! The JSON Schema of the client link's frames, written to `web/`, which
//! generates its Effect Schemas from it (ADR 0002). The `schemas` suite
//! runs this, regenerates the Effect Schemas and fails on any difference
//! from what is committed.

use std::path::PathBuf;

use schemars::generate::SchemaSettings;
use serde_json::{Value, json};

use crate::{CommandResult, Feed, HelloReply, Open};

fn path() -> PathBuf {
    [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        "web",
        "src",
        "proto",
    ]
    .iter()
    .collect::<PathBuf>()
    .join("proto.schema.json")
}

/// Every frame on the link and every type they reach, in `$defs`, as
/// serde writes them.
fn document() -> Value {
    let settings = SchemaSettings::draft2020_12().for_serialize();
    let meta = settings.meta_schema.clone();
    let mut generator = settings.into_generator();
    generator.subschema_for::<Open>();
    generator.subschema_for::<HelloReply>();
    generator.subschema_for::<CommandResult>();
    generator.subschema_for::<Feed>();
    json!({
        "$schema": meta,
        "$defs": generator.take_definitions(true),
    })
}

/// Rewritten only when it no longer reads as the frames' document, so a
/// formatter's layout of it stays.
#[test]
fn the_frames_schema_is_written_for_the_web_app()
-> Result<(), Box<dyn std::error::Error>> {
    let path = path();
    let document = document();
    let written = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    if written.as_ref() != Some(&document) {
        std::fs::create_dir_all(path.parent().ok_or("no parent")?)?;
        std::fs::write(&path, serde_json::to_vec_pretty(&document)?)?;
    }
    Ok(())
}
