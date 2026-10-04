//! Build and run the decryptor from hash-verified dependency copies.
use crate::sha256;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path},
    process::Command,
};

#[derive(Deserialize)]
struct Dependency {
    files: BTreeMap<String, String>,
    c_sources: Vec<String>,
    cpp_sources: Vec<String>,
}
fn run(command: &mut Command) -> Result<()> {
    let result = command.output().context("run native PRX helper build")?;
    ensure!(
        result.status.success(),
        "native helper failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(())
}
pub(super) fn decrypt(
    root: &Path,
    external: &Path,
    encrypted: &[u8],
    temp: &Path,
) -> Result<Value> {
    let config = fs::read(root.join("config/prx-decryptor.json"))?;
    let dep: Dependency = serde_json::from_slice(&config)?;
    let copied = temp.join("dependency");
    for (name, expected) in &dep.files {
        ensure!(
            Path::new(name)
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
            "invalid dependency path"
        );
        let raw =
            fs::read(external.join(name)).with_context(|| format!("read dependency {name}"))?;
        ensure!(
            sha256(&raw) == *expected,
            "PRX dependency identity mismatch: {name}"
        );
        let target = copied.join(name);
        fs::create_dir_all(target.parent().context("dependency parent")?)?;
        fs::write(target, raw)?;
    }
    // Compile only the verified copies, never source that can change after checking.
    let helper = include_bytes!("../support/decrypt_prx.cpp");
    let helper_path = temp.join("helper.cpp");
    fs::write(&helper_path, helper)?;
    let mut objects = Vec::new();
    for (index, (compiler, name)) in dep
        .c_sources
        .iter()
        .map(|n| ("cc", n))
        .chain(dep.cpp_sources.iter().map(|n| ("c++", n)))
        .enumerate()
    {
        ensure!(
            dep.files.contains_key(name),
            "unverified compilation source"
        );
        let object = temp.join(format!("source-{index}.o"));
        let mut command = Command::new(compiler);
        if compiler == "c++" {
            command.arg("-std=c++17");
        }
        run(command
            .args(["-O2", "-I"])
            .arg(&copied)
            .arg("-c")
            .arg(copied.join(name))
            .arg("-o")
            .arg(&object))?;
        objects.push(object);
    }
    let executable = temp.join("decrypt-prx");
    run(Command::new("c++")
        .args(["-std=c++17", "-O2", "-I"])
        .arg(&copied)
        .arg(&helper_path)
        .args(&objects)
        .arg("-o")
        .arg(&executable))?;
    let input = temp.join("encrypted.bin");
    fs::write(&input, encrypted)?;
    run(Command::new(&executable)
        .arg(input)
        .arg(temp.join("plain.elf")))?;
    let cc = Command::new("cc").arg("--version").output()?;
    let cxx = Command::new("c++").arg("--version").output()?;
    Ok(
        json!({"profile_sha256":sha256(&config), "helper_sha256":sha256(helper),
        "binary_sha256":sha256(&fs::read(executable)?), "dependency_files":dep.files.len(),
        "cc":String::from_utf8_lossy(&cc.stdout).lines().next(), "cxx":String::from_utf8_lossy(&cxx.stdout).lines().next()}),
    )
}
