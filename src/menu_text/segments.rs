//! Prose-only replacement around immutable, parsed source controls.
use super::*;

pub(super) fn encode(
    entry: &Value,
    source: &Value,
    group: usize,
    record: usize,
    map: &BTreeMap<char, u16>,
    allowed: &[u16],
    line_cells: usize,
) -> Result<Vec<u16>> {
    ensure!(
        entry["group"] == group && entry["record"] == record,
        "segment record order"
    );
    ensure!(
        entry["source_units"] == source["units"] && entry["source_text"] == source["text"],
        "segment source drift"
    );
    let controls = controls(source)?;
    let spans = entry["segments"].as_array().context("prose segments")?;
    ensure!(
        spans.len() == controls.len() + 1,
        "segment/control population"
    );
    let mut units = Vec::new();
    let mut preview = String::new();
    let mut terminated = false;
    let mut width = 0;
    for (i, span) in spans.iter().enumerate() {
        let span = span.as_str().context("prose segment")?;
        ensure!(!terminated || span.is_empty(), "prose after terminator");
        for c in span.chars() {
            ensure!(!c.is_control(), "prose contains control character");
            units.push(*map.get(&c).context("unmapped segment character")?);
            width += 1;
            ensure!(width <= line_cells, "segment line width exceeded");
        }
        preview.push_str(span);
        if let Some(control) = controls.get(i) {
            let code = control[0];
            ensure!(
                allowed.contains(&code),
                "unadopted segment control {code:04X}"
            );
            ensure!(!terminated || code == 0xffff, "control after terminator");
            units.extend(control);
            preview.push_str(&format!("<{code:04X}"));
            for operand in &control[1..] {
                preview.push_str(&format!(":{operand:04X}"));
            }
            preview.push('>');
            // F812 restores both cursor coordinates from the origin (08895EA0).
            // F813 only changes wait state and must not reset accumulated width.
            if matches!(code, 0xfffd | 0xf812) {
                width = 0;
            }
            terminated |= code == 0xffff;
        }
    }
    ensure!(
        terminated && entry["expected_text"] == preview,
        "segment translation alignment"
    );
    Ok(units)
}

fn controls(record: &Value) -> Result<Vec<Vec<u16>>> {
    let mut result = Vec::new();
    for token in record["tokens"]
        .as_array()
        .context("unresolved source tokens")?
    {
        if let Some(code) = token.get("control") {
            let mut units = vec![u16::try_from(code.as_u64().context("control")?)?];
            for operand in token["operands"].as_array().context("operands")? {
                units.push(u16::try_from(operand.as_u64().context("operand")?)?);
            }
            result.push(units);
        }
    }
    Ok(result)
}

pub(super) fn verify(entry: &Value, source: &Value, output: &Value) -> Result<()> {
    let expected = entry["expected_text"].as_str().context("expected text")?;
    let actual = output["text"].as_str().context("unresolved output text")?;
    let extra = actual
        .strip_prefix(expected)
        .context("segment output text mismatch")?;
    ensure!(
        extra.replace("<FFFF>", "").is_empty(),
        "non-padding output tail"
    );
    let source_controls = controls(source)?;
    let output_controls = controls(output)?;
    ensure!(
        output_controls.starts_with(&source_controls)
            && output_controls[source_controls.len()..]
                .iter()
                .all(|c| c == &[0xffff]),
        "segment output control drift"
    );
    Ok(())
}
