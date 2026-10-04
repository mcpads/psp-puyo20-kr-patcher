//! Apply reviewed Korean wording to translation JSON without touching source controls.
//!
//! A fix names a file, a JSON path (`$.entries[3]`), the reviewed old text and the new text,
//! both with `/` for source line breaks. Story entries keep one line per `<FFFD>` control and
//! spread each line over the spans between other controls (waits, expressions) in proportion
//! to their old lengths. School entries carry colour spans, so only span-local edits are
//! accepted there. Entries without inline source text accept deletions only.
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Clone, Deserialize, serde::Serialize)]
pub struct Fix {
    pub file: String,
    pub path: String,
    pub old: String,
    pub new: String,
}

fn locate<'a>(doc: &'a mut Value, path: &str) -> Result<&'a mut Value> {
    let mut rest = path.strip_prefix('$').context("path must start with $")?;
    let mut obj = doc;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('.') {
            let end = r.find(['.', '[']).unwrap_or(r.len());
            obj = obj.get_mut(&r[..end]).context("missing key")?;
            rest = &r[end..];
        } else if let Some(r) = rest.strip_prefix('[') {
            let end = r.find(']').context("unclosed index")?;
            let i: usize = r[..end].parse()?;
            obj = obj.get_mut(i).context("missing index")?;
            rest = &r[end + 1..];
        } else {
            bail!("bad path {path}");
        }
    }
    Ok(obj)
}

/// `<XXXX>` or `<XXXX:YYYY>` control tokens in source order: (token text, code).
pub(crate) fn controls(source: &str) -> Vec<(String, String)> {
    let b = source.as_bytes();
    let hex = |s: &[u8]| s.len() == 4 && s.iter().all(|c| matches!(c, b'0'..=b'9' | b'A'..=b'F'));
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' && i + 6 <= b.len() && hex(&b[i + 1..i + 5]) {
            let end = if b[i + 5] == b'>' {
                Some(i + 6)
            } else if b[i + 5] == b':'
                && i + 11 <= b.len()
                && hex(&b[i + 6..i + 10])
                && b[i + 10] == b'>'
            {
                Some(i + 11)
            } else {
                None
            };
            if let Some(end) = end {
                out.push((source[i..end].to_string(), source[i + 1..i + 5].to_string()));
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Split `text` over spans in proportion to `weights`, preferring cuts after spaces.
/// Spans after the last weighted one (e.g. behind the end-wait control) stay empty.
pub(crate) fn split_proportional(text: &str, weights: &[usize]) -> Vec<String> {
    let n = weights.len();
    if n == 1 || weights.iter().all(|w| *w == 0) {
        let mut v = vec![text.to_string()];
        v.resize(n, String::new());
        return v;
    }
    let last = weights.iter().rposition(|w| *w > 0).unwrap_or(0);
    let weights = &weights[..=last];
    let mut parts = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    for (i, &w) in weights[..weights.len() - 1].iter().enumerate() {
        if w == 0 {
            parts.push(String::new());
            continue;
        }
        let remaining: usize = weights[i..].iter().sum();
        let target = ((rest.len() * w) as f64 / remaining as f64).round() as usize;
        let mut cut = target;
        let spaces: Vec<usize> = (0..rest.len())
            .filter(|&k| rest[k] == ' ')
            .map(|k| k + 1)
            .collect();
        if let Some(&best) = spaces.iter().min_by_key(|p| p.abs_diff(target))
            && best.abs_diff(target) <= 3.max(rest.len() / 3)
        {
            cut = best;
        }
        let cut = cut.min(rest.len());
        parts.push(rest[..cut].iter().collect());
        rest = rest[cut..].to_vec();
    }
    parts.push(rest.into_iter().collect());
    parts.resize(n, String::new());
    parts
}

fn strings(v: &Value) -> Result<Vec<String>> {
    v.as_array()
        .context("string list")?
        .iter()
        .map(|s| Ok(s.as_str().context("string")?.to_string()))
        .collect()
}

fn apply_story(entry: &mut Value, new: &str) -> Result<()> {
    let Some(source) = entry.get("source_text").and_then(Value::as_str) else {
        return delete_only(entry, new);
    };
    let ctrls = controls(source);
    let mut segs = strings(&entry["segments"])?;
    ensure!(segs.len() == ctrls.len() + 1, "segment/control mismatch");
    let mut lines: Vec<Vec<usize>> = vec![Vec::new()];
    for i in 0..segs.len() {
        lines.last_mut().unwrap().push(i);
        if ctrls.get(i).is_some_and(|c| c.1 == "FFFD") {
            lines.push(Vec::new());
        }
    }
    let mut new_lines: Vec<&str> = new.split('/').collect();
    // A trailing group may hold only control padding (after the end wait).
    while new_lines.len() < lines.len()
        && lines[new_lines.len()..]
            .iter()
            .all(|g| g.iter().all(|&i| segs[i].trim().is_empty()))
    {
        new_lines.push("");
    }
    ensure!(
        new_lines.len() == lines.len(),
        "line count {} != source lines {}",
        new.split('/').count(),
        lines.len()
    );
    let old = segs.clone();
    for (idx, text) in lines.iter().zip(new_lines) {
        let mut weights: Vec<usize> = idx.iter().map(|&i| old[i].chars().count()).collect();
        if weights.iter().all(|w| *w == 0) {
            weights[0] = 1;
        }
        for (&i, part) in idx.iter().zip(split_proportional(text, &weights)) {
            segs[i] = part;
        }
    }
    let mut prose = String::new();
    for (i, s) in segs.iter().enumerate() {
        prose.push_str(s);
        if ctrls.get(i).is_some_and(|c| c.1 == "FFFD") {
            prose.push('\n');
        }
    }
    entry["segments"] = json!(segs);
    entry["expected_text"] = json!(prose);
    Ok(())
}

/// Entries without inline source: pure character deletions mapped onto segments.
fn delete_only(entry: &mut Value, new: &str) -> Result<()> {
    let expected = entry["expected_text"]
        .as_str()
        .context("expected_text")?
        .to_string();
    let old: Vec<char> = expected.replace('\n', "/").chars().collect();
    let new: Vec<char> = new.chars().collect();
    let mut keep = Vec::with_capacity(old.len());
    let mut j = 0;
    for ch in &old {
        let k = j < new.len() && new[j] == *ch;
        keep.push(k);
        j += usize::from(k);
    }
    ensure!(
        j == new.len(),
        "only deletions are supported without inline source text"
    );
    let mut flags = old
        .iter()
        .zip(&keep)
        .filter(|(c, _)| **c != '/')
        .map(|(_, k)| *k);
    let segs: Vec<String> = strings(&entry["segments"])?
        .iter()
        .map(|s| s.chars().filter(|_| flags.next().unwrap_or(true)).collect())
        .collect();
    let text: String = expected
        .chars()
        .zip(&keep)
        .filter(|(c, k)| **k || *c == '\n')
        .map(|(c, _)| c)
        .collect();
    entry["segments"] = json!(segs);
    entry["expected_text"] = json!(text);
    Ok(())
}

fn apply_academy(entry: &mut Value, old: &str, new: &str) -> Result<()> {
    let source = entry["source_text"]
        .as_str()
        .context("source_text")?
        .to_string();
    let ctrls = controls(&source);
    let mut segs: Vec<Vec<char>> = strings(&entry["segments"])?
        .iter()
        .map(|s| s.chars().collect())
        .collect();
    ensure!(segs.len() == ctrls.len() + 1, "segment/control mismatch");
    let mut lines: Vec<Vec<(usize, usize)>> = vec![Vec::new()];
    for (i, s) in segs.iter().enumerate() {
        lines
            .last_mut()
            .unwrap()
            .extend((0..s.len()).map(|k| (i, k)));
        if ctrls.get(i).is_some_and(|c| c.1 == "FFFD") {
            lines.push(Vec::new());
        }
    }
    let text = |ln: &[(usize, usize)]| -> Vec<char> {
        ln.iter()
            .map(|&(i, k)| {
                if segs[i][k] == '\u{3000}' {
                    ' '
                } else {
                    segs[i][k]
                }
            })
            .collect()
    };
    let old_lines: Vec<&str> = old.split('/').collect();
    let new_lines: Vec<&str> = new.split('/').collect();
    ensure!(
        old_lines.len() == new_lines.len(),
        "academy line count changed"
    );
    let actual: Vec<Vec<char>> = lines.iter().map(|l| text(l)).collect();
    let first = actual
        .iter()
        .position(|t| t.iter().any(|c| !c.is_whitespace()))
        .context("empty entry")?;
    let mut edits = Vec::new();
    for (j, (a, b)) in old_lines.iter().zip(&new_lines).enumerate() {
        if a == b {
            continue;
        }
        let ln = lines.get(first + j).context("line outside entry")?;
        let full = &actual[first + j];
        let a2: Vec<char> = a.trim().chars().collect();
        let b2: Vec<char> = b.trim().chars().collect();
        let lead = (0..=full.len().saturating_sub(a2.len()))
            .find(|&s| full[s..].starts_with(&a2))
            .with_context(|| format!("academy line {j} differs from reviewed text"))?;
        let mut p = 0;
        while p < a2.len().min(b2.len()) && a2[p] == b2[p] {
            p += 1;
        }
        let mut q = 0;
        while q < a2.len().min(b2.len()) - p && a2[a2.len() - 1 - q] == b2[b2.len() - 1 - q] {
            q += 1;
        }
        let old_mid = a2.len() - q - p;
        let new_mid: Vec<char> = b2[p..b2.len() - q].to_vec();
        let pos = lead + p;
        let (si, off) = if old_mid > 0 {
            let span = &ln[pos..pos + old_mid];
            ensure!(
                span.iter().all(|s| s.0 == span[0].0),
                "academy edit crosses a control span"
            );
            span[0]
        } else {
            let (si, off) = *ln
                .get(pos.checked_sub(1).context("insertion at line start")?)
                .context("insertion")?;
            (si, off + 1)
        };
        edits.push((si, off, old_mid, new_mid));
    }
    edits.sort_by_key(|e| (e.0, std::cmp::Reverse(e.1)));
    for (si, off, n, rep) in edits {
        segs[si].splice(off..off + n, rep);
    }
    let mut out = String::new();
    for (i, s) in segs.iter().enumerate() {
        out.extend(s.iter());
        if let Some((token, _)) = ctrls.get(i) {
            out.push_str(token);
        }
    }
    entry["segments"] = json!(
        segs.iter()
            .map(|s| s.iter().collect::<String>())
            .collect::<Vec<_>>()
    );
    entry["expected_text"] = json!(out);
    Ok(())
}

fn apply_lines(entry: &mut Value, new: &str) -> Result<()> {
    let old = strings(&entry["lines"])?;
    let core_len = old.iter().rposition(|l| !l.is_empty()).map_or(0, |p| p + 1);
    let mut lines: Vec<String> = new.split('/').map(str::to_string).collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    // Keep the source layout's indentation and trailing empty lines.
    if lines.len() == core_len {
        for (l, o) in lines.iter_mut().zip(&old) {
            let indent: String = o.chars().take_while(|c| c.is_whitespace()).collect();
            *l = format!("{indent}{}", l.trim_start());
        }
    }
    lines.extend(old[core_len..].iter().cloned());
    entry["lines"] = json!(lines);
    Ok(())
}

fn apply(entry: &mut Value, fix: &Fix) -> Result<()> {
    if entry.get("lines").is_some() {
        apply_lines(entry, &fix.new)
    } else if entry.get("segments").is_some() && fix.file.contains("/story/") {
        apply_story(entry, &fix.new)
    } else if entry.get("segments").is_some() {
        apply_academy(entry, &fix.old, &fix.new)
    } else if entry.get("korean").is_some_and(Value::is_string) {
        entry["korean"] = json!(fix.new);
        Ok(())
    } else if entry.get("ko").is_some_and(Value::is_string) {
        entry["ko"] = json!(fix.new);
        Ok(())
    } else {
        bail!("unsupported entry shape")
    }
}

/// Serialize with the indentation and trailing newline the file already uses.
pub fn dump_like(old: &str, doc: &Value) -> Result<String> {
    let original: Value = serde_json::from_str(old)?;
    for indent in [&b"  "[..], b" ", b"    "] {
        let render = |v: &Value| -> Result<String> {
            let mut buf = Vec::new();
            let fmt = serde_json::ser::PrettyFormatter::with_indent(indent);
            let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
            serde::Serialize::serialize(v, &mut ser)?;
            Ok(String::from_utf8(buf)?)
        };
        let base = render(&original)?;
        for tail in ["\n", ""] {
            if format!("{base}{tail}") == old {
                return Ok(format!("{}{tail}", render(doc)?));
            }
        }
    }
    bail!("unknown JSON layout")
}

pub fn run(root: &Path, fixes: &[Fix], dry_run: bool) -> Result<Value> {
    let mut docs: BTreeMap<String, (String, Value)> = BTreeMap::new();
    let mut applied = 0;
    let mut refused = Vec::new();
    for fix in fixes {
        if !docs.contains_key(&fix.file) {
            let text = fs::read_to_string(root.join(&fix.file))?;
            let doc = serde_json::from_str(&text)?;
            docs.insert(fix.file.clone(), (text, doc));
        }
        let (_, doc) = docs.get_mut(&fix.file).unwrap();
        let result = locate(doc, &fix.path).and_then(|entry| {
            let before = entry.clone();
            let r = apply(entry, fix);
            if r.is_err() {
                *entry = before;
            }
            r
        });
        match result {
            Ok(()) => applied += 1,
            Err(e) => refused.push(json!({"file":fix.file,"path":fix.path,"old":fix.old,"new":fix.new,"reason":e.to_string()})),
        }
    }
    let mut changed = Vec::new();
    for (file, (text, doc)) in &docs {
        let out = dump_like(text, doc).with_context(|| file.clone())?;
        if out != *text {
            changed.push(file.clone());
            if !dry_run {
                fs::write(root.join(file), out)?;
            }
        }
    }
    Ok(json!({"applied":applied,"refused":refused,"changed_files":changed,"dry_run":dry_run}))
}

#[cfg(test)]
mod tests;
