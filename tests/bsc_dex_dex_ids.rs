//! Файл: tests/bsc_dex_dex_ids.rs
//! Что делает: решение 128 — список id области «DEX» для интерфейса (pools/dex_ids.rs::merge) на живых пулах
//! GeckoTerminal 2026-10-24 (tests/data/indexers_2026-10-24): строки с видом остаются как есть, к ним добавляется
//! каждая DEX с пулами в справочнике (вид None) и id с уже заданными настройками, без повторов, по id.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::dex_ids, directory::gt_parse, gateway::contract::DexFamily.
//! Настройки: нет.

use std::collections::BTreeSet;

use arb::core::types::PoolKind;
use arb::gateway::contract::DexFamily;
use arb::venues::bsc_dex::directory::gt_parse;
use arb::venues::bsc_dex::pools::dex_ids::{Source, merge};
use serde_json::Value;

/// dex_id всех пулов из записанных ответов индексатора по монетам.
fn recorded_pool_dex_ids() -> BTreeSet<String> {
    let text =
        std::fs::read_to_string("tests/data/indexers_2026-10-24/arb_indexers_2026-10-24.json")
            .expect("файл записи");
    let all: Value = serde_json::from_str(&text).expect("JSON");
    let mut ids = BTreeSet::new();
    for (name, rec) in all.as_object().unwrap() {
        if !name.starts_with("gt_pools_") {
            continue;
        }
        let pools = gt_parse::pools(rec["body"].as_str().unwrap()).expect("разбор пулов");
        ids.extend(pools.into_iter().map(|p| p.dex_id));
    }
    ids
}

/// Строка с видом (как от for_ui).
fn row(id: &str, kind: PoolKind) -> DexFamily {
    DexFamily {
        dex: id.into(),
        family: id.into(),
        kind: Some(kind),
        title: format!("{id} с видом"),
        docs: Vec::new(),
    }
}

#[test]
fn merge_adds_directory_and_settings_ids() {
    let dir = recorded_pool_dex_ids();
    // Живые id из записи: «thena-v3» (пулы ETH и DOGE) и основные DEX — есть.
    for id in ["thena-v3", "pancakeswap-v3-bsc", "uniswap-v4-bsc"] {
        assert!(dir.contains(id), "{id} в записи");
    }
    let known = vec![
        row("pancakeswap-v3-bsc", PoolKind::V3),
        row("uniswap-v4-bsc", PoolKind::Cl),
    ];
    let mut extra: Vec<(String, Source, Option<String>)> = dir
        .iter()
        .map(|id| (id.clone(), Source::Directory, None))
        .collect();
    // DEX, которой нет в справочнике: только настройки (введена руками), и id с неподдержанным видом.
    extra.push(("lista-stable".into(), Source::Settings, None));
    extra.push((
        "thena-v3".into(),
        Source::Settings,
        Some("unsupported".into()),
    ));
    // Повтор строки с видом — не дублируется и не теряет вид.
    extra.push(("pancakeswap-v3-bsc".into(), Source::Settings, None));

    let out = merge(known, extra);
    let ids: Vec<&str> = out.iter().map(|f| f.family.as_str()).collect();
    let uniq: BTreeSet<&str> = ids.iter().copied().collect();
    assert_eq!(ids.len(), uniq.len(), "без повторов");
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "по id");
    for id in &dir {
        assert!(
            uniq.contains(id.as_str()),
            "{id} из справочника есть в списке"
        );
    }
    assert!(uniq.contains("lista-stable"));
    let pcs = out
        .iter()
        .find(|f| f.family == "pancakeswap-v3-bsc")
        .unwrap();
    assert_eq!(pcs.kind, Some(PoolKind::V3));
    assert_eq!(pcs.title, "pancakeswap-v3-bsc с видом");
    let thena = out.iter().find(|f| f.family == "thena-v3").unwrap();
    assert_eq!(thena.kind, None);
    assert!(
        thena.title.contains("пулы в справочнике"),
        "{}",
        thena.title
    );
    assert!(thena.title.contains("«unsupported»"), "{}", thena.title);
    let lista = out.iter().find(|f| f.family == "lista-stable").unwrap();
    assert!(lista.title.contains("есть настройки") && lista.title.contains("dex.kind"));
}
