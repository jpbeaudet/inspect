//! `inspect alias` — manage saved selector aliases.

use crate::alias;
use crate::cli::{AliasArgs, AliasCommand};
use crate::error::ExitKind;

pub fn run(args: AliasArgs) -> anyhow::Result<ExitKind> {
    match args.command {
        AliasCommand::Add(a) => add(a),
        AliasCommand::List(a) => list(a),
        AliasCommand::Remove(a) => remove(a),
        AliasCommand::Show(a) => show(a),
    }
}

fn add(a: crate::cli::AliasAddArgs) -> anyhow::Result<ExitKind> {
    alias::add(&a.name, &a.selector, a.description, a.force)?;
    let kind = alias::classify(&a.selector);
    let params = alias::extract_parameters(&a.selector);
    println!(
        "SUMMARY: alias '@{}' saved ({}-style)",
        a.name,
        kind.label()
    );
    println!("DATA:    selector = {}", a.selector);
    if !params.is_empty() {
        println!("         parameters = [{}]", params.join(", "));
    }
    if params.is_empty() {
        println!("NEXT:    use '@{}' wherever a selector is accepted", a.name);
    } else {
        let example = params
            .iter()
            .map(|p| format!("{p}=..."))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "NEXT:    call as @{}({example}) wherever a selector is accepted",
            a.name
        );
    }
    Ok(ExitKind::Success)
}

fn list(a: crate::cli::AliasListArgs) -> anyhow::Result<ExitKind> {
    let entries = alias::list()?;
    if a.format.is_json() {
        let arr: Vec<_> = entries
            .iter()
            .map(|(n, e)| {
                let params = e
                    .parameters
                    .clone()
                    .unwrap_or_else(|| alias::extract_parameters(&e.selector));
                serde_json::json!({
                    "name": n,
                    "selector": e.selector,
                    "description": e.description,
                    "kind": alias::classify(&e.selector).label(),
                    "parameters": params,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::Value::Array(arr))?
        );
        return Ok(ExitKind::Success);
    }
    if entries.is_empty() {
        println!("SUMMARY: no aliases configured");
        println!("DATA:    (none)");
        println!("NEXT:    add one with 'inspect alias add <name> <selector>'");
        return Ok(ExitKind::Success);
    }
    println!("SUMMARY: {} alias(es) configured", entries.len());
    println!("DATA:");
    for (name, entry) in &entries {
        let kind = alias::classify(&entry.selector).label();
        let params = entry
            .parameters
            .clone()
            .unwrap_or_else(|| alias::extract_parameters(&entry.selector));
        let params_tag = if params.is_empty() {
            String::new()
        } else {
            format!(" ({})", params.join(","))
        };
        let desc = entry
            .description
            .as_deref()
            .map(|d| format!(" — {d}"))
            .unwrap_or_default();
        println!(
            "  @{name}{params_tag} [{kind}] = {}{}",
            entry.selector, desc
        );
    }
    println!("NEXT:    'inspect alias show <name>' for full detail");
    Ok(ExitKind::Success)
}

fn remove(a: crate::cli::AliasRemoveArgs) -> anyhow::Result<ExitKind> {
    let removed = alias::remove(&a.name)?;
    if removed {
        println!("SUMMARY: alias '@{}' removed", a.name);
        println!("DATA:    -");
        println!("NEXT:    'inspect alias list' to see remaining aliases");
        Ok(ExitKind::Success)
    } else {
        println!("SUMMARY: alias '@{}' did not exist", a.name);
        println!("DATA:    -");
        println!("NEXT:    'inspect alias list' to see configured aliases");
        Ok(ExitKind::Error)
    }
}

fn show(a: crate::cli::AliasShowArgs) -> anyhow::Result<ExitKind> {
    let Some(entry) = alias::get(&a.name)? else {
        println!("SUMMARY: alias '@{}' is not defined", a.name);
        println!("DATA:    -");
        println!("NEXT:    'inspect alias list' to see configured aliases");
        return Ok(ExitKind::Error);
    };
    let kind = alias::classify(&entry.selector);
    let params = entry
        .parameters
        .clone()
        .unwrap_or_else(|| alias::extract_parameters(&entry.selector));
    if a.format.is_json() {
        let defaults = alias::extract_defaults(&entry.selector);
        let v = serde_json::json!({
            "name": a.name,
            "selector": entry.selector,
            "description": entry.description,
            "kind": kind.label(),
            "parameters": params,
            "parameter_defaults": defaults,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(ExitKind::Success);
    }
    println!("SUMMARY: alias '@{}' ({}-style)", a.name, kind.label());
    println!("DATA:    selector    = {}", entry.selector);
    if !params.is_empty() {
        println!("         parameters  = [{}]", params.join(", "));
    }
    if let Some(d) = entry.description {
        println!("         description = {d}");
    }
    if params.is_empty() {
        println!("NEXT:    use '@{}' wherever a selector is accepted", a.name);
    } else {
        let example = params
            .iter()
            .map(|p| format!("{p}=..."))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "NEXT:    call as @{}({example}) wherever a selector is accepted",
            a.name
        );
    }
    Ok(ExitKind::Success)
}
