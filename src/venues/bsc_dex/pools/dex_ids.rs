//! Файл: venues/bsc_dex/pools/dex_ids.rs
//! Что делает: решение 128 — полный список id области «DEX» для интерфейса (контракт dex_families). Раньше в нём были
//! только DEX с поддержанным видом (dex.kind задан), а dex.kind можно задать только для DEX из этого списка —
//! замкнутый круг: новое семейство (Maverick V2, PancakeSwap Stable, Lista, Nerve, WooFi, Infinity Stable, Ellipsis)
//! не включить. Теперь к строкам с видом добавляются: (а) каждая DEX, чьи пулы есть в справочнике пулов (dex_id пула),
//! (б) каждый id, у которого уже задана хоть одна настройка области «DEX». У добавленных вид — None (не задан или не
//! поддерживается), в названии — почему. DEX, которой нет ни там, ни там, человек вводит сам (кнопка «Добавить DEX»
//! в карточке настройки) — после первой сохранённой настройки она попадает сюда по (б).
//! Кто вызывает: pools/dex_rules.rs (for_ui); tests/bsc_dex_dex_ids.rs (merge).
//! От чего зависит: gateway/contract (DexFamily), directory/view.rs (snapshot), settings (Scope), ctx.rs.
//! Настройки: читает dex.kind [DEX] (вид не поддерживается — в названии) и все значения области «DEX».

use std::collections::BTreeMap;

use crate::gateway::contract::DexFamily;
use crate::settings::scope::Scope;

use super::super::ctx::Ctx;
use super::super::directory;

/// Откуда id попал в список (для названия строки).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// Пулы этой DEX есть в справочнике пулов (индексатор, события создания, поиск по фабрикам).
    Directory,
    /// У id уже есть настройки области «DEX» (например введён руками, решение 128).
    Settings,
}

/// Что делает: сводит строки с видом и добавочные id в один список без повторов, по id.
/// Принимает: строки с видом (for_ui), добавочные id с источником и заданным, но не поддержанным dex.kind (если есть).
/// Возвращает: строки с видом как есть + по строке на каждый новый id (kind = None). Ошибок нет.
pub fn merge(
    known: Vec<DexFamily>,
    extra: impl IntoIterator<Item = (String, Source, Option<String>)>,
) -> Vec<DexFamily> {
    let mut out: BTreeMap<String, DexFamily> =
        known.into_iter().map(|f| (f.family.clone(), f)).collect();
    // Одна строка на id: источник «справочник» сильнее «настроек» (для названия).
    let mut add: BTreeMap<String, (Source, Option<String>)> = BTreeMap::new();
    for (id, src, kind) in extra {
        if out.contains_key(&id) {
            continue;
        }
        let e = add.entry(id).or_insert((src, kind.clone()));
        e.0 = e.0.min(src);
        if e.1.is_none() {
            e.1 = kind;
        }
    }
    for (id, (src, kind)) in add {
        out.insert(id.clone(), extra_row(id, src, kind));
    }
    out.into_values().collect()
}

/// Строка без вида: название говорит, откуда id и что задать. Принимает: id, источник, заданный dex.kind.
/// Возвращает: DexFamily с kind = None. Ошибок нет.
fn extra_row(id: String, src: Source, kind: Option<String>) -> DexFamily {
    let from = match src {
        Source::Directory => "пулы в справочнике",
        Source::Settings => "есть настройки",
    };
    let what = match kind {
        Some(k) => format!("вид «{k}» не поддерживается или не подключён"),
        None => "вид не задан (dex.kind)".to_string(),
    };
    DexFamily {
        dex: id.clone(),
        family: id.clone(),
        kind: None,
        title: format!("{id} — {from}, {what}"),
        docs: Vec::new(),
    }
}

/// Что делает: добавочные id из справочника пулов и из настроек области «DEX».
/// Принимает: контекст адаптера. Возвращает: [(id, источник, dex.kind текстом, если задан)]. Ошибок нет.
pub fn extra(ctx: &Ctx) -> Vec<(String, Source, Option<String>)> {
    let kind = |id: &str| ctx.dex_text("dex.kind", id).ok();
    let mut out: Vec<(String, Source, Option<String>)> = directory::snapshot(ctx)
        .pools
        .into_iter()
        .map(|p| p.dex_id)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|id| {
            let k = kind(&id);
            (id, Source::Directory, k)
        })
        .collect();
    for (_, scope, _) in ctx.settings.all() {
        if let Scope::Dex(id) = scope {
            let k = kind(&id);
            out.push((id, Source::Settings, k));
        }
    }
    out
}
