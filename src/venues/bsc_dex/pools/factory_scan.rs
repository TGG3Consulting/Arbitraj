//! Файл: venues/bsc_dex/pools/factory_scan.rs
//! Что делает: поиск пулов по фабрикам на своём узле (решение 121, C3) — второй источник справочника пулов рядом с
//! индексатором (решение 58 дополнено). Включён index.factory_scan [bsc_dex] (не задана — выключен): по каждой монете
//! подписки и каждому встречному токену (USDT и промежуточные, решение 52) — вызовы реестра фабрик подключённых DEX
//! (factory_calls.rs) пачками Multicall3 по dex.load_batch на одном теге latest. Найденный пул (ненулевой адрес) —
//! в справочник (поле factory) и в ту же проверку на своём узле и тот же реестр, что пулы индексатора: монета уходит
//! в канал «монета обновлена» (index_watch.rs → index::take → verify → attach). Фабрика даёт только адрес.
//! Проход — при старте подписки и после изменения index.factory_scan или dex.* (без опроса); часть пачки без ответа
//! узла — повтор прохода через dex.verify_retry_s. Найденное не стирается (пары у фабрики не удаляются); выключили —
//! справочник фабрик очищается (уже подключённые пулы слушаются до новой подписки).
//! Кто вызывает: pools/index.rs (begin — задача рядом с обновлением индексатора).
//! От чего зависит: ctx.rs, factory_calls.rs, node/call.rs (try_chunks: Multicall3), directory (Snap), settings/live.rs.
//! Настройки: index.factory_scan, dex.load_batch, dex.verify_retry_s [bsc_dex]; через factory_calls — dex.factory,
//! dex.v3_fee_tiers [DEX].

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use alloy_primitives::Address;
use chrono::Utc;
use tokio::sync::mpsc::UnboundedSender;

use crate::core::types::FactoryScan;
use crate::gateway::errors::ErrorCode;
use crate::settings::live::Watch;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::directory::gt_parse::IndexedPool;
use super::super::node::call;
use super::factory_calls::{self, Plan};

/// Работает, пока жива подписка. Принимает: окружение, монеты подписки (адрес → тикер, с промежуточными),
/// встречные токены, канал «монета обновлена». Ошибок не возвращает: всё — в журнал и в справочник (интерфейс).
pub async fn run(
    ctx: Ctx,
    coins: HashMap<Address, String>,
    quotes: HashSet<Address>,
    tx: UnboundedSender<Address>,
) {
    let names = vec!["index.factory_scan".to_string(), "dex.".to_string()];
    let mut w = Watch::new(ctx.settings.subscribe(), names);
    loop {
        let retry = pass(&ctx, &coins, &quotes, &tx).await;
        let pause = async {
            match retry {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = pause => {}
            c = w.next() => if c.is_none() { return },
        }
    }
}

/// Один проход. Возвращает: через сколько повторить (часть вызовов без ответа узла и задана dex.verify_retry_s);
/// None — ждать изменения настроек. Ошибок нет.
async fn pass(
    ctx: &Ctx,
    coins: &HashMap<Address, String>,
    quotes: &HashSet<Address>,
    tx: &UnboundedSender<Address>,
) -> Option<Duration> {
    match ctx.flag("index.factory_scan") {
        Ok(true) => {}
        Ok(false) => return off(ctx, None),
        Err(e) if e.code == ErrorCode::NotConfigured => return off(ctx, None),
        Err(e) => return off(ctx, Some(e.message)),
    }
    let mut coin_list: Vec<Address> = coins.keys().copied().collect();
    coin_list.sort();
    let mut quote_list: Vec<Address> = quotes.iter().copied().collect();
    quote_list.sort();
    let plan = match factory_calls::plan(ctx, &coin_list, &quote_list) {
        Ok(p) => p,
        Err(e) => return failed(ctx, e.message).await,
    };
    let size = match ctx.int("dex.load_batch") {
        Ok(n) => n as usize,
        Err(e) => return failed(ctx, e.message).await,
    };
    let answers = call::try_chunks(ctx, &plan.calls, "latest", size).await;
    let (found, mut notes, no_answer) = sort(&plan, answers, coins);
    notes.splice(0..0, plan.notes.iter().cloned());
    let total: usize = found.values().map(Vec::len).sum();
    let fresh = ctx.index.with(|s| {
        let mut fresh = Vec::new();
        for (coin, list) in found {
            let have = s.factory.entry(coin).or_default();
            let before = have.len();
            for p in list {
                if !have.iter().any(|x| x.pool == p.pool) {
                    have.push(p);
                }
            }
            if have.len() > before {
                fresh.push(coin);
            }
        }
        s.factory_scan = FactoryScan {
            enabled: true,
            at: Some(Utc::now()),
            calls: plan.calls.len(),
            found: s.factory.values().map(Vec::len).sum(),
            taken: 0,
            failed: no_answer,
            notes,
            error: None,
        };
        fresh
    });
    let msg = format!(
        "Поиск пулов по фабрикам: вызовов {}, пулов в ответах {total}, монет с новыми пулами {}, без ответа узла {no_answer}",
        plan.calls.len(),
        fresh.len()
    );
    ctx.log(Level::Info, "dex.index", msg, None).await;
    for coin in fresh {
        let _ = tx.send(coin);
    }
    if no_answer > 0 {
        return ctx.int("dex.verify_retry_s").ok().map(Duration::from_secs);
    }
    None
}

/// Найденные пулы по монете, заметки (откаты, не по ABI), число вызовов без ответа узла.
type Sorted = (HashMap<Address, Vec<IndexedPool>>, Vec<String>, usize);

/// Разбирает ответы по вопросам. Принимает: план, ответы по порядку, тикеры. Возвращает: Sorted. Ошибок нет.
fn sort(plan: &Plan, answers: Vec<call::Piece>, coins: &HashMap<Address, String>) -> Sorted {
    let sym = |a: Address| coins.get(&a).cloned().unwrap_or_else(|| format!("{a:#x}"));
    let mut found: HashMap<Address, Vec<IndexedPool>> = HashMap::new();
    let (mut reverted, mut bad, mut no_answer) = (0, 0, 0);
    let mut first_bad = None;
    for (ask, a) in plan.asks.iter().zip(answers) {
        let pool = match a {
            Ok(Ok(b)) => match factory_calls::decode(ask.fee, &b) {
                Ok(p) => p,
                Err(why) => {
                    bad += 1;
                    first_bad.get_or_insert(format!("{}: {why}", ask.family.id));
                    continue;
                }
            },
            Ok(Err(_)) => {
                reverted += 1;
                continue;
            }
            Err(_) => {
                no_answer += 1;
                continue;
            }
        };
        if pool == Address::ZERO {
            continue;
        }
        found.entry(ask.coin).or_default().push(IndexedPool {
            dex_id: ask.family.id.to_string(),
            pool: format!("{pool:#x}"),
            base: ask.coin,
            quote: ask.quote,
            base_symbol: sym(ask.coin),
            quote_symbol: sym(ask.quote),
            reserve_usd: None,
        });
    }
    let mut notes = Vec::new();
    if reverted > 0 {
        notes.push(format!(
            "вызовов откатилось: {reverted} — фабрика не отвечает getPair/getPool (проверьте dex.factory и dex.kind)"
        ));
    }
    if let Some(b) = first_bad {
        notes.push(format!("ответов не по ABI: {bad} (первый — {b})"));
    }
    (found, notes, no_answer)
}

/// Поиск выключен (или выключатель не читается): справочник фабрик очищается. Возвращает: None. Ошибок нет.
fn off(ctx: &Ctx, error: Option<String>) -> Option<Duration> {
    ctx.index.with(|s| {
        s.factory.clear();
        s.factory_scan = FactoryScan {
            error,
            ..FactoryScan::default()
        };
    });
    None
}

/// Проход не выполнен (не задана настройка): причина — в справочник и в журнал при смене. Возвращает: None.
async fn failed(ctx: &Ctx, why: String) -> Option<Duration> {
    let changed = ctx.index.with(|s| {
        s.factory_scan.enabled = true;
        let was = s.factory_scan.error.replace(why.clone());
        was.as_deref() != Some(why.as_str())
    });
    if changed {
        let msg = format!("Поиск пулов по фабрикам не выполнен: {why}");
        ctx.log(Level::Warn, "dex.index", msg, None).await;
    }
    None
}
