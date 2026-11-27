//! Файл: venues/bsc_dex/pools/factory_scan.rs
//! Что делает: поиск пулов по фабрикам на своём узле (решение 121, C3) — второй источник справочника пулов рядом с
//! индексатором (решение 58 дополнено). Включён index.factory_scan [bsc_dex] (не задана — выключен): по каждой монете
//! подписки и каждому встречному токену (USDT и промежуточные, решение 52) — вызовы реестра фабрик подключённых DEX
//! (factory_calls.rs; виды вопросов — factory_ask.rs: V2, V3, Solidly, Algebra, DODO V2 — решение 122 F) пачками Multicall3 по dex.load_batch на одном теге latest. Найденный пул (ненулевой адрес) —
//! в справочник (поле factory) и в ту же проверку на своём узле и тот же реестр, что пулы индексатора: монета уходит
//! в канал «монета обновлена» (index_watch.rs → index::take → verify → attach). Фабрика даёт только адрес.
//! Проход — при старте подписки и после изменения index.factory_scan или dex.* (без опроса; пачка изменений — один
//! проход по итоговым настройкам, решение 279); часть без ответа узла — повтор через dex.verify_retry_s. Найденное не
//! стирается (пары у фабрики не удаляются); выключили — справочник фабрик очищается (подключённые пулы слушаются).
//! Решение 449: начало прохода — в итоге (FactoryScan.started) и в журнал: список DEX и их настройки читаются в начале
//! прохода, итог (at, заметки) пишется в конце — без времени начала заметка «не подключена» у DEX, подключённой во
//! время долгого прохода, выглядела как ошибка настроек. Изменение dex.* во время прохода даёт следующий проход.
//! Кто вызывает: pools/index.rs (begin — задача рядом с обновлением индексатора).
//! От чего зависит: ctx.rs, factory_calls.rs, woo_quote.rs, node/call.rs (try_chunks: Multicall3), directory (Snap), settings/live.rs.
//! Решение 305: до плана — quoteToken() контрактов WooFi с dex.multi_pair = да (woo_quote.rs read) — в план.
//! Настройки: index.factory_scan, dex.load_batch, dex.verify_retry_s [bsc_dex]; через factory_calls — dex.factory,
//! dex.v3_fee_tiers [DEX]; через stable_list.rs — dex.stable_pools [DEX] (решение 292: пулы Saddle / Curve 2020 из
//! списка — кандидаты без индексатора); через metric_list.rs — dex.factory вида metric_omm (решение 482: пулы Metric
//! перечислением фабрики nextPoolIdx / idxToPool — реестра по паре у неё нет).

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
use super::factory_ask::{Q, decode_q};
use super::factory_calls::{self, Plan};
use super::metric_list;
use super::stable_list;
use super::woo_quote;

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
            c = w.next_batch() => if c.is_none() { return },
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
    let started = Utc::now();
    ctx.index.with(|s| {
        s.factory_scan.enabled = true;
        s.factory_scan.started = Some(started);
    });
    let msg = "Поиск пулов по фабрикам: проход начат (DEX и их настройки — на этот момент)";
    ctx.log(Level::Info, "dex.index", msg.to_string(), None)
        .await;
    let mut coin_list: Vec<Address> = coins.keys().copied().collect();
    coin_list.sort();
    let mut quote_list: Vec<Address> = quotes.iter().copied().collect();
    quote_list.sort();
    let size = match ctx.int("dex.load_batch") {
        Ok(n) => n as usize,
        Err(e) => return failed(ctx, e.message).await,
    };
    // Решение 305: quoteToken() контрактов WooFi (dex.multi_pair = да) с цепи — до плана (пара WooFi — монета и этот
    // встречный); режим не включён — пусто, план как раньше.
    let (woo, woo_notes, woo_no_answer) = woo_quote::read(ctx, size).await;
    // У каждого вызова quoteToken() ровно один итог: прочитан, заметка (откат / не по ABI / ноль) или без ответа узла.
    let woo_calls = woo.len() + woo_notes.len() + woo_no_answer;
    let plan = match factory_calls::plan(ctx, &coin_list, &quote_list, &woo) {
        Ok(p) => p,
        Err(e) => return failed(ctx, e.message).await,
    };
    let answers = call::try_chunks(ctx, &plan.calls, "latest", size).await;
    let (mut found, mut notes, mut no_answer) = sort(&plan, answers, coins);
    notes.splice(0..0, plan.notes.iter().cloned());
    notes.splice(0..0, woo_notes);
    no_answer += woo_no_answer;
    // Решение 292: пулы StableSwap из dex.stable_pools (Saddle, Curve 2020) — кандидаты по монетам самого пула.
    let mut extra = stable_list::scan(ctx, coins, &quote_list, size).await;
    Box::pin(super::ambient_directory::scan(ctx,coins,&quote_list,&mut extra)).await;
    super::euler_directory::extend(ctx,coins,&quote_list,&mut extra);
    super::infinity_seeds::extend(ctx, coins, &quote_list, &mut extra);
    for (coin, list) in extra.found {
        found.entry(coin).or_default().extend(list);
    }
    notes.extend(extra.notes);
    no_answer += extra.no_answer;
    // Решение 482: пулы Metric OMM — перечислением фабрики, кандидаты по монетам пула (как список выше).
    let metric = metric_list::scan(ctx, coins, &quote_list, size).await;
    for (coin, list) in metric.found {
        found.entry(coin).or_default().extend(list);
    }
    notes.extend(metric.notes);
    no_answer += metric.no_answer;
    let ll=Box::pin(super::llamma_directory::scan(ctx,coins,&quote_list)).await;
    for (coin,rows) in ll.found {found.entry(coin).or_default().extend(rows);}
    notes.extend(ll.notes);no_answer+=ll.no_answer;
    let yb=Box::pin(super::yield_basis_directory::scan(ctx,coins,&quote_list)).await;
    for (coin,rows) in yb.found {found.entry(coin).or_default().extend(rows);}
    notes.extend(yb.notes);no_answer+=yb.no_answer;
    let bv1=Box::pin(super::balancer_v1_directory::scan(ctx,coins,&quote_list)).await;
    for (coin,rows) in bv1.found {found.entry(coin).or_default().extend(rows);}
    notes.extend(bv1.notes);no_answer+=bv1.no_answer;
    let aqua=super::aqua_directory::scan(ctx,coins,&quote_list);
    for (coin, rows) in aqua.found {found.entry(coin).or_default().extend(rows);}
    notes.extend(aqua.notes);
    let legacy=Box::pin(super::bancor_legacy_directory::scan(ctx,coins,&quote_list)).await;
    for (coin,rows) in legacy.found {found.entry(coin).or_default().extend(rows);}
    notes.extend(legacy.notes); no_answer+=legacy.no_answer;
    let bancor=Box::pin(super::bancor_directory::scan(ctx,coins,&quote_list)).await;
    for (coin,rows) in bancor.found {found.entry(coin).or_default().extend(rows);}
    notes.extend(bancor.notes);no_answer+=bancor.no_answer;
    let mav=Box::pin(super::maverick_one_directory::scan(ctx,coins,&quote_list)).await;
    for (coin,rows) in mav.found{found.entry(coin).or_default().extend(rows);}
    notes.extend(mav.notes);no_answer+=mav.no_answer;
    let carbon=super::carbon_directory::scan(ctx,coins,&quote_list).await;
    for (coin,rows) in carbon.found{found.entry(coin).or_default().extend(rows);}
    notes.extend(carbon.notes);no_answer+=carbon.no_answer;
    let brokers=super::brokerbot_directory::scan(ctx,coins,&quote_list);
    for (coin,rows) in brokers.found{found.entry(coin).or_default().extend(rows);}
    let temple=Box::pin(super::temple_directory::scan(ctx,coins,&quote_list)).await;
    for (coin,rows) in temple.found{found.entry(coin).or_default().extend(rows);}
    notes.extend(temple.notes);no_answer+=temple.no_answer;
    let ranges=Box::pin(super::puzzle_ranges::scan(ctx,coins,&quote_list)).await;
    for (coin,rows) in ranges.found {found.entry(coin).or_default().extend(rows);}
    notes.extend(ranges.notes); no_answer+=ranges.no_answer;
    let prop=super::prop_venue::directory(ctx,coins,&quote_list);
    for (coin,rows) in prop.found{found.entry(coin).or_default().extend(rows);}
    let integral=Box::pin(super::integral_directory::scan(ctx,coins,&quote_list)).await;
    for (coin,rows) in integral.found{found.entry(coin).or_default().extend(rows);}
    notes.extend(integral.notes);no_answer+=integral.no_answer;
    let pendle=super::pendle::scan(ctx,coins,&quote_list);
    for (coin,rows) in pendle.found{found.entry(coin).or_default().extend(rows);}
    let surge=super::surge::scan(ctx,coins,&quote_list);
    for (coin,rows) in surge.found{found.entry(coin).or_default().extend(rows);}
    let plaza=Box::pin(super::defiplaza::scan(ctx,coins,&quote_list)).await;
    for (coin,rows) in plaza.found{found.entry(coin).or_default().extend(rows);}
    notes.extend(plaza.notes);no_answer+=plaza.no_answer;
    let dag=Box::pin(super::dagrow::scan(ctx,coins,&quote_list)).await;
    for (coin,rows) in dag.found{found.entry(coin).or_default().extend(rows);}
    notes.extend(dag.notes);no_answer+=dag.no_answer;
    let n_calls = bv1.calls + bancor.calls + integral.calls + plaza.calls + dag.calls + temple.calls + carbon.calls + plan.calls.len() + extra.calls + metric.calls + woo_calls;
    let total: usize = found.values().map(Vec::len).sum();
    let fresh = ctx.index.with(|s| {
        let mut fresh = Vec::new();
        for (coin, list) in found {
            // Решение 295: у DEX в режиме dex.multi_pair строки одного пула с разными парами — разные (dir_key); у
            // прочих ключ — строка пула, как до решения.
            let multi = s.multi_dex.clone();
            let key = |x: &IndexedPool| {
                super::multi_pair::dir_key(&multi, &x.dex_id, &x.pool, x.base, x.quote)
            };
            let have = s.factory.entry(coin).or_default();
            let before = have.len();
            for p in list {
                if !have.iter().any(|x| key(x) == key(&p)) {
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
            started: Some(started),
            calls: n_calls,
            found: s.factory.values().map(Vec::len).sum(),
            taken: 0,
            failed: no_answer,
            notes,
            error: None,
        };
        fresh
    });
    let msg = format!(
        "Поиск пулов по фабрикам: вызовов {n_calls}, пулов в ответах {total}, монет с новыми пулами {}, без ответа узла {no_answer}",
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
        let pools = match a {
            Ok(Ok(b)) => match decode_q(ask.q, &b) {
                Ok(p) => p,
                Err(why) => {
                    bad += 1;
                    first_bad.get_or_insert(format!("{}: {why}", ask.family.id));
                    continue;
                }
            },
            // Решение 442 (ревью): откат find_pool_for_coins при i > 0 — у Twocrypto-NG номер за последним пулом пары
            // (DynArray, factory_ask.rs), не ошибка; откат при i = 0 — в «вызовов откатилось» (неверная фабрика).
            Ok(Err(_)) if matches!(ask.q, Q::CurveCrypto(i) if i > 0) => continue,
            Ok(Err(_)) => {
                reverted += 1;
                continue;
            }
            Err(_) => {
                no_answer += 1;
                continue;
            }
        };
        // Нулевые адреса (пула нет) уже отброшены decode_q; у DODO в одном ответе может быть несколько пулов пары.
        for pool in pools {
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
    }
    let mut notes = Vec::new();
    if reverted > 0 {
        notes.push(format!(
            "вызовов откатилось: {reverted} — фабрика не отвечает на вызов реестра своего вида (проверьте dex.factory и dex.kind)"
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

/// Проход не выполнен (не задана настройка): причина — в справочник и в журнал при смене; начало прохода
/// снимается (ревью 449: иначе экран показывал «Идёт проход» до следующего успешного). Возвращает: None.
async fn failed(ctx: &Ctx, why: String) -> Option<Duration> {
    let changed = ctx.index.with(|s| {
        s.factory_scan.enabled = true;
        s.factory_scan.started = None;
        let was = s.factory_scan.error.replace(why.clone());
        was.as_deref() != Some(why.as_str())
    });
    if changed {
        let msg = format!("Поиск пулов по фабрикам не выполнен: {why}");
        ctx.log(Level::Warn, "dex.index", msg, None).await;
    }
    None
}
