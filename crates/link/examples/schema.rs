//! Prints the JSON Schema of the client link's frames, which
//! `tools/schemas.sh` writes beside `link` as `link.schema.json` for a
//! client to generate its types from (ADR 0002): every frame on the link,
//! the ticket a client reaches a host by and every type they reach, in
//! `$defs`, as serde writes them, and the link's ALPN as `Alpn`.

use std::io::Write;

use link::{ALPN, CommandResult, Feed, HelloReply, Open, Ticket};
use schemars::generate::SchemaSettings;
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let settings = SchemaSettings::draft2020_12().for_serialize();
    let meta = settings.meta_schema.clone();
    let mut generator = settings.into_generator();
    generator.subschema_for::<Open>();
    generator.subschema_for::<HelloReply>();
    generator.subschema_for::<CommandResult>();
    generator.subschema_for::<Feed>();
    generator.subschema_for::<Ticket>();
    let mut definitions = generator.take_definitions(true);
    definitions.insert(
        String::from("Alpn"),
        json!({
            "description": "The ALPN a client dials a host with.",
            "type": "string",
            "const": std::str::from_utf8(ALPN)?,
        }),
    );
    let document = json!({ "$schema": meta, "$defs": definitions });
    std::io::stdout()
        .write_all(serde_json::to_string_pretty(&document)?.as_bytes())?;
    Ok(())
}
