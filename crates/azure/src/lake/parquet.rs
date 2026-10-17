//! Parquet without downloading it. A file ends in its metadata (a Thrift
//! compact-encoded `FileMetaData`), the metadata's 4-byte little-endian
//! length and `PAR1`, so one ranged read of the tail (two for a footer past
//! 64 KiB) is all `path get`, `table get` and `partition list` read. Only
//! the schema, the row count and the writer are decoded; every other field
//! is skipped by its wire type. Field ids are parquet-format's
//! `parquet.thrift`; the encoding is Thrift's compact protocol.

use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::parallel;
use crate::lake::{Lake, Ref};

/// One tail read holds the footer of all but the widest files.
const TAIL: u64 = 64 * 1024;
/// Deeper than any real schema; a bound on what a corrupt footer can ask.
const DEPTH: usize = 64;

/// A top-level column, typed as Parquet types it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Column {
    pub name: String,
    /// `INT64`, `STRING`, `DECIMAL(12,2)`, `DATE`, `TIMESTAMP(MICROS, UTC)`,
    /// `STRUCT`, `LIST`, `MAP`.
    #[serde(rename = "type")]
    pub kind: String,
}

/// What a Parquet footer says.
#[derive(Clone, Debug)]
pub(crate) struct Footer {
    pub rows: u64,
    /// The library that wrote it (`created_by`).
    pub writer: Option<String>,
    pub schema: Vec<Column>,
}

/// Each file's footer (`files`: each one's id and size), read side by side;
/// one that cannot be read says why.
pub(crate) fn footers(lake: &Lake, files: &[(Ref, u64)]) -> Vec<Result<Footer>> {
    parallel(files, lake.azure.parallel(), |(file, size)| {
        tail(lake, file, *size)
            .and_then(|metadata| parse(&metadata))
            .with_context(|| format!("{} is not a Parquet file it can read", file.id()))
    })
}

/// The file's metadata: the bytes before its length and magic.
fn tail(lake: &Lake, file: &Ref, size: u64) -> Result<Vec<u8>> {
    if size < 12 {
        bail!("{size} bytes is too small");
    }
    let url = file.url();
    let want = size.min(TAIL);
    let mut held = lake.range(&url, size - want, size - 1)?;
    if held.len() as u64 != want {
        bail!("asked for {want} bytes, got {}", held.len());
    }
    match &held[held.len() - 4..] {
        b"PAR1" => {}
        b"PARE" => bail!("its footer is encrypted"),
        _ => bail!("no PAR1 at its end"),
    }
    let length = u64::from(u32::from_le_bytes(
        held[held.len() - 8..held.len() - 4].try_into()?,
    ));
    if length + 12 > size {
        bail!("its footer is longer than the file");
    }
    if length + 8 > want {
        let mut more = lake.range(&url, size - length - 8, size - want - 1)?;
        more.extend_from_slice(&held);
        held = more;
    }
    let end = held.len() - 8;
    Ok(held[end - usize::try_from(length)?..end].to_vec())
}

/// The rows, writer and top-level columns `metadata` (a `FileMetaData`)
/// holds.
pub(crate) fn parse(metadata: &[u8]) -> Result<Footer> {
    let mut reader = Reader {
        bytes: metadata,
        at: 0,
    };
    let (mut schema, mut rows, mut writer) = (Vec::new(), None, None);
    let mut last = 0;
    while let Some((id, kind)) = reader.field(&mut last)? {
        match (id, kind) {
            (2, LIST) => {
                let (count, element) = reader.list()?;
                if element != STRUCT {
                    bail!("its schema is not a list of elements");
                }
                for _ in 0..count {
                    schema.push(reader.element()?);
                }
            }
            (3, I64) => rows = Some(reader.zigzag()?),
            (6, BINARY) => writer = Some(String::from_utf8_lossy(reader.binary()?).into_owned()),
            (_, kind) => reader.skip(kind, 0)?,
        }
    }
    let rows = rows.context("its footer has no row count")?;
    let root = schema.first().context("its footer has no schema")?;
    let mut columns = Vec::new();
    let mut at = 1;
    for _ in 0..root.children {
        let column = schema.get(at).context("its schema ends early")?;
        columns.push(Column {
            name: column.name.clone(),
            kind: column.kind(),
        });
        at = past(&schema, at, 0)?;
    }
    Ok(Footer {
        rows: u64::try_from(rows).unwrap_or(0),
        writer,
        schema: columns,
    })
}

/// The index just past element `at` and everything nested in it.
fn past(schema: &[Element], at: usize, depth: usize) -> Result<usize> {
    if depth > DEPTH {
        bail!("its schema nests too deep");
    }
    let held = schema.get(at).context("its schema ends early")?;
    let mut next = at + 1;
    for _ in 0..held.children {
        next = past(schema, next, depth + 1)?;
    }
    Ok(next)
}

/// A `SchemaElement`, as much of it as names a type.
#[derive(Debug, Default)]
struct Element {
    name: String,
    physical: Option<i64>,
    length: Option<i64>,
    children: i64,
    converted: Option<i64>,
    scale: Option<i64>,
    precision: Option<i64>,
    logical: Option<String>,
}

impl Element {
    /// Its type as Parquet's logical types name it, else its converted
    /// type, else its physical one.
    fn kind(&self) -> String {
        if self.children > 0 {
            return match (self.logical.as_deref(), self.converted) {
                (Some("LIST"), _) | (_, Some(3)) => "LIST",
                (Some("MAP"), _) | (_, Some(1 | 2)) => "MAP",
                _ => "STRUCT",
            }
            .to_owned();
        }
        if let Some(logical) = &self.logical {
            return logical.clone();
        }
        let converted = match self.converted {
            Some(0) => Some("STRING".to_owned()),
            Some(4) => Some("ENUM".to_owned()),
            Some(5) => Some(format!(
                "DECIMAL({},{})",
                self.precision.unwrap_or(0),
                self.scale.unwrap_or(0)
            )),
            Some(6) => Some("DATE".to_owned()),
            Some(7) => Some("TIME(MILLIS)".to_owned()),
            Some(8) => Some("TIME(MICROS)".to_owned()),
            Some(9) => Some("TIMESTAMP(MILLIS)".to_owned()),
            Some(10) => Some("TIMESTAMP(MICROS)".to_owned()),
            Some(code @ 11..=14) => Some(format!("UINT{}", 8 << (code - 11))),
            Some(code @ 15..=18) => Some(format!("INT{}", 8 << (code - 15))),
            Some(19) => Some("JSON".to_owned()),
            Some(20) => Some("BSON".to_owned()),
            Some(21) => Some("INTERVAL".to_owned()),
            _ => None,
        };
        converted.unwrap_or_else(|| match self.physical {
            Some(0) => "BOOLEAN".to_owned(),
            Some(1) => "INT32".to_owned(),
            Some(2) => "INT64".to_owned(),
            Some(3) => "INT96".to_owned(),
            Some(4) => "FLOAT".to_owned(),
            Some(5) => "DOUBLE".to_owned(),
            Some(6) => "BINARY".to_owned(),
            Some(7) => format!("FIXED({})", self.length.unwrap_or(0)),
            _ => "UNKNOWN".to_owned(),
        })
    }
}

// Thrift compact protocol wire types.
const TRUE: u8 = 1;
const FALSE: u8 = 2;
const BYTE: u8 = 3;
const I16: u8 = 4;
const I32: u8 = 5;
const I64: u8 = 6;
const DOUBLE: u8 = 7;
const BINARY: u8 = 8;
const LIST: u8 = 9;
const SET: u8 = 10;
const MAP: u8 = 11;
const STRUCT: u8 = 12;
const UUID: u8 = 13;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .context("its footer ends early")?;
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn varint(&mut self) -> Result<u64> {
        let mut value = 0_u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        bail!("a varint runs past 64 bits")
    }

    fn zigzag(&mut self) -> Result<i64> {
        let raw = self.varint()?;
        Ok((raw >> 1) as i64 ^ -((raw & 1) as i64))
    }

    fn binary(&mut self) -> Result<&'a [u8]> {
        let length = usize::try_from(self.varint()?)?;
        self.take(length)
    }

    /// The next field's id and wire type, or `None` at its struct's end.
    /// `last` is the struct's previous field id, which a short header's
    /// delta counts from.
    fn field(&mut self, last: &mut i64) -> Result<Option<(i64, u8)>> {
        let header = self.byte()?;
        if header == 0 {
            return Ok(None);
        }
        let delta = i64::from(header >> 4);
        let id = if delta == 0 {
            self.zigzag()?
        } else {
            *last + delta
        };
        *last = id;
        Ok(Some((id, header & 0x0f)))
    }

    /// A list or set header: its size and its elements' wire type.
    fn list(&mut self) -> Result<(usize, u8)> {
        let header = self.byte()?;
        let size = match header >> 4 {
            15 => usize::try_from(self.varint()?)?,
            short => usize::from(short),
        };
        Ok((size, header & 0x0f))
    }

    /// Skips a value of wire type `kind`. A boolean in a list or map is a
    /// byte of its own; one in a struct field lives in its header.
    fn skip(&mut self, kind: u8, depth: usize) -> Result<()> {
        if depth > DEPTH {
            bail!("its footer nests too deep");
        }
        let element = |reader: &mut Self, kind: u8| match kind {
            TRUE | FALSE => reader.byte().map(drop),
            other => reader.skip(other, depth + 1),
        };
        match kind {
            TRUE | FALSE => {}
            BYTE => {
                self.byte()?;
            }
            I16 | I32 | I64 => {
                self.varint()?;
            }
            DOUBLE => {
                self.take(8)?;
            }
            BINARY => {
                self.binary()?;
            }
            LIST | SET => {
                let (count, inner) = self.list()?;
                for _ in 0..count {
                    element(self, inner)?;
                }
            }
            MAP => {
                let count = self.varint()?;
                if count > 0 {
                    let types = self.byte()?;
                    for _ in 0..count {
                        element(self, types >> 4)?;
                        element(self, types & 0x0f)?;
                    }
                }
            }
            STRUCT => {
                let mut last = 0;
                while let Some((_, inner)) = self.field(&mut last)? {
                    self.skip(inner, depth + 1)?;
                }
            }
            UUID => {
                self.take(16)?;
            }
            other => bail!("wire type {other} is not Thrift's compact protocol"),
        }
        Ok(())
    }

    fn element(&mut self) -> Result<Element> {
        let mut element = Element::default();
        let mut last = 0;
        while let Some((id, kind)) = self.field(&mut last)? {
            match (id, kind) {
                (1, I32) => element.physical = Some(self.zigzag()?),
                (2, I32) => element.length = Some(self.zigzag()?),
                (4, BINARY) => element.name = String::from_utf8_lossy(self.binary()?).into_owned(),
                (5, I32) => element.children = self.zigzag()?.max(0),
                (6, I32) => element.converted = Some(self.zigzag()?),
                (7, I32) => element.scale = Some(self.zigzag()?),
                (8, I32) => element.precision = Some(self.zigzag()?),
                (10, STRUCT) => element.logical = self.logical()?,
                (_, kind) => self.skip(kind, 1)?,
            }
        }
        Ok(element)
    }

    /// A `LogicalType` union's name with its parameters; `None` for one
    /// this does not name, which falls back to the converted type.
    fn logical(&mut self) -> Result<Option<String>> {
        let mut named = None;
        let mut last = 0;
        while let Some((id, kind)) = self.field(&mut last)? {
            if kind != STRUCT {
                self.skip(kind, 1)?;
                continue;
            }
            let fields = self.parameters()?;
            let int = |at: i64| {
                fields
                    .iter()
                    .find(|(id, _)| *id == at)
                    .map(|(_, value)| *value)
            };
            let unit = match int(2) {
                Some(1) => "MILLIS",
                Some(3) => "NANOS",
                _ => "MICROS",
            };
            let utc = if int(1) == Some(1) { ", UTC" } else { "" };
            named = match id {
                1 => Some("STRING".to_owned()),
                2 => Some("MAP".to_owned()),
                3 => Some("LIST".to_owned()),
                4 => Some("ENUM".to_owned()),
                5 => Some(format!(
                    "DECIMAL({},{})",
                    int(2).unwrap_or(0),
                    int(1).unwrap_or(0)
                )),
                6 => Some("DATE".to_owned()),
                7 => Some(format!("TIME({unit}{utc})")),
                8 => Some(format!("TIMESTAMP({unit}{utc})")),
                10 => Some(format!(
                    "{}INT{}",
                    if int(2) == Some(1) { "" } else { "U" },
                    int(1).unwrap_or(0)
                )),
                12 => Some("JSON".to_owned()),
                13 => Some("BSON".to_owned()),
                14 => Some("UUID".to_owned()),
                15 => Some("FLOAT16".to_owned()),
                _ => None,
            };
        }
        Ok(named)
    }

    /// A logical type's parameters, flattened to each field's id and value:
    /// a boolean as 1 or 0, a time unit union as its member's id (1 millis,
    /// 2 micros, 3 nanos).
    fn parameters(&mut self) -> Result<Vec<(i64, i64)>> {
        let mut fields = Vec::new();
        let mut last = 0;
        while let Some((id, kind)) = self.field(&mut last)? {
            let value = match kind {
                TRUE => 1,
                FALSE => 0,
                BYTE => i64::from(self.byte()?.cast_signed()),
                I16 | I32 | I64 => self.zigzag()?,
                STRUCT => {
                    let (mut member, mut inner) = (0, 0);
                    while let Some((id, kind)) = self.field(&mut inner)? {
                        member = id;
                        self.skip(kind, 2)?;
                    }
                    member
                }
                other => {
                    self.skip(other, 2)?;
                    0
                }
            };
            fields.push((id, value));
        }
        Ok(fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lake::testing::world;

    /// The metadata in a whole file's bytes.
    fn metadata(file: &[u8]) -> &[u8] {
        let length = u32::from_le_bytes(file[file.len() - 8..file.len() - 4].try_into().unwrap());
        &file[file.len() - 8 - length as usize..file.len() - 8]
    }

    fn types(footer: Footer) -> Vec<(String, String)> {
        footer
            .schema
            .into_iter()
            .map(|column| (column.name, column.kind))
            .collect()
    }

    fn named(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    #[test]
    fn a_footer_gives_the_rows_writer_and_parquet_types_of_the_top_level_columns() {
        let orders = world("contosolake/raw/sales/orders/date=2026-09-29/part-0001.parquet");
        let footer = parse(metadata(&orders)).unwrap();
        assert_eq!(footer.rows, 7);
        assert!(footer.writer.clone().unwrap().starts_with("DuckDB"));
        assert_eq!(
            types(footer),
            named(&[
                ("order_id", "INT64"),
                ("customer_id", "INT64"),
                ("total", "DOUBLE"),
                ("placed_at", "TIMESTAMP(MICROS, UTC)"),
            ])
        );
        let ledger = world("contosolake/curated/finance/ledger/part-0000.parquet");
        let footer = parse(metadata(&ledger)).unwrap();
        assert_eq!(footer.rows, 120);
        assert_eq!(
            types(footer),
            named(&[
                ("entry_id", "INT64"),
                ("account", "STRING"),
                ("amount", "DECIMAL(12,2)"),
                ("posted", "DATE"),
            ])
        );
    }

    #[test]
    fn a_nested_column_is_one_column_and_a_cut_footer_is_an_error_not_a_panic() {
        // FileMetaData { schema: [root (2 children), s (1 child: x INT32), y BOOLEAN], num_rows: 3 }
        let element = |name: &str, children: Option<u8>, physical: Option<u8>| {
            let mut out = Vec::new();
            if let Some(physical) = physical {
                out.extend([0x15, physical * 2]);
            }
            // name is field 4: a delta of 3 after the type, else of 4.
            out.extend([if physical.is_some() { 0x38 } else { 0x48 }]);
            out.push(u8::try_from(name.len()).unwrap());
            out.extend(name.as_bytes());
            if let Some(children) = children {
                out.extend([0x15, children * 2]);
            }
            out.push(0);
            out
        };
        let mut footer = vec![0x29, 0x4c];
        footer.extend(element("root", Some(2), None));
        footer.extend(element("s", Some(1), None));
        footer.extend(element("x", None, Some(1)));
        footer.extend(element("y", None, Some(0)));
        footer.extend([0x16, 6, 0]);
        let parsed = parse(&footer).unwrap();
        assert_eq!(parsed.rows, 3);
        assert_eq!(types(parsed), named(&[("s", "STRUCT"), ("y", "BOOLEAN")]));
        for cut in 0..footer.len() {
            assert!(parse(&footer[..cut]).is_err(), "cut at {cut}");
        }
        assert!(parse(&[0x19, 0xfc, 0xff, 0xff, 0xff, 0x0f]).is_err());
    }
}
