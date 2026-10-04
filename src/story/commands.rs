//! Observed PSS commands and literal MTX candidates; no execution interpreter.
use crate::text;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

struct Word<'a> {
    text: &'a str,
    offset: usize,
    line: usize,
}

pub(super) fn scan_commands(raw: &[u8], context: bool) -> Result<Vec<Value>> {
    let script = raw.strip_suffix(&[0]).context("PSS missing final NUL")?;
    ensure!(
        script.is_ascii() && !script.contains(&0),
        "unsupported PSS encoding"
    );
    let script = std::str::from_utf8(script)?;
    let mut words = Vec::new();
    let mut start = 0;
    for (index, line) in script.split_inclusive('\n').enumerate() {
        let mut pos = 0;
        while pos < line.len() {
            if line.as_bytes()[pos].is_ascii_whitespace() {
                pos += 1;
                continue;
            }
            let begin = pos;
            while pos < line.len() && !line.as_bytes()[pos].is_ascii_whitespace() {
                pos += 1;
            }
            words.push(Word {
                text: &line[begin..pos],
                offset: start + begin,
                line: index + 1,
            });
        }
        start += line.len();
    }
    let mut section = None;
    let mut calls = Vec::new();
    for (i, word) in words.iter().enumerate() {
        if word.text == ":" {
            section = Some(words.get(i + 1).context("missing section name")?.text);
        }
        if context && word.text == "MzEntryCharTextData" {
            let end = words[i + 1..]
                .iter()
                .position(|w| w.text == ";")
                .map(|n| i + 1 + n)
                .context("unterminated character text table")?;
            let args = &words[i + 1..end];
            ensure!(
                !args.is_empty() && args.len().is_multiple_of(3),
                "character text table requires complete triples"
            );
            let values = args
                .iter()
                .map(|w| {
                    w.text
                        .parse::<i32>()
                        .context("non-numeric character text table")
                })
                .collect::<Result<Vec<_>>>()?;
            // The observed table has three integers per row. Keep columns opaque:
            // neither character-ID lookup nor argument semantics are proven here.
            calls.push(json!({"command":word.text,"line":word.line,
                "byte_offset":word.offset,"section":section,
                "arguments":args.iter().map(|w|w.text).collect::<Vec<_>>(),
                "source_call":&script[word.offset..words[end].offset+1],
                "table_rows":values.as_chunks::<3>().0,
                "mapping_status":"lookup_unresolved"}));
            continue;
        }
        let arity = match word.text {
            "MzSetText" => 6,
            "MzLoadChr" | "MzEnterChr" | "MzChangeName" | "MzGetGameCharID" if context => 2,
            "MzGetCharTextData" if context => 4,
            "MzDeleteChr" | "MzExitChr" | "MzExitChrOpp" | "MzExitChrFall" if context => 1,
            _ => continue,
        };
        let args = words
            .get(i + 1..i + 1 + arity)
            .context("truncated story command")?;
        ensure!(
            args.iter().all(|a| a.text.parse::<i32>().is_ok()
                || (a.text.as_bytes()[0].is_ascii_alphabetic()
                    && a.text
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_'))),
            "unsupported story command argument"
        );
        // Preserve unknown argument meanings and source order. Multiple commands
        // on one line are legal in the observed files, so line splitting alone
        // must never be used to enumerate calls.
        calls.push(
            json!({"command":word.text,"line":word.line,"byte_offset":word.offset,
            "section":section,"arguments":args.iter().map(|w|w.text).collect::<Vec<_>>(),
            "source_call": &script[word.offset..args[arity-1].offset+args[arity-1].text.len()]}),
        );
    }
    Ok(calls)
}

pub(super) fn scan_calls(raw: &[u8]) -> Result<Vec<Value>> {
    scan_commands(raw, false)
}

pub(super) fn candidate_record(
    call: &Value,
    mtx: &text::Mtx,
) -> Result<Option<(usize, usize, usize)>> {
    let args = call["arguments"].as_array().context("call arguments")?;
    let (Ok(group), Ok(record)) = (
        args[0].as_str().context("group")?.parse::<i32>(),
        args[1].as_str().context("record")?.parse::<i32>(),
    ) else {
        return Ok(None);
    };
    ensure!(group >= 0 && record >= 0, "negative literal text reference");
    let group = group as usize;
    let record = record as usize;
    let first = *mtx
        .group_first_records
        .get(group)
        .context("text group out of bounds")?;
    let end = mtx
        .group_first_records
        .get(group + 1)
        .copied()
        .unwrap_or(mtx.records.len());
    ensure!(record < end - first, "text record outside group");
    Ok(Some((group, record, first + record)))
}
