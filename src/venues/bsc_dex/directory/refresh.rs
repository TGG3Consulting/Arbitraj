//! Файл: venues/bsc_dex/directory/refresh.rs
//! Что делает: обновление справочника пулов от индексатора (решение 58) — задача подписки на рынок. По каждой монете
//! подписки (и промежуточной монете) — пулы монеты страницами (не больше index.pages_per_token; страница короче
//! index.page_size — последняя), не чаще раза в index.refresh_s; список DEX сети — так же раз в index.refresh_s,
//! по links.next. После перезапуска — из SQLite (store.rs): свежие монеты не спрашиваются. Каждую обновлённую монету
//! получает слушатель (канал `tx`) — проверка на своём узле и подключение пулов. Ошибка запроса монеты не стирает
//! прежний ответ (пол-списка — не факт): видна в интерфейсе, монета спрашивается в следующий раз.
//! Кто вызывает: pools/market.rs (задача подписки; при новой подписке прерывается).
//! От чего зависит: ctx.rs, directory/{fetch,gt_http,store,mod}.rs, settings/live.rs.
//! Настройки: index.refresh_s, index.pages_per_token, index.page_size [bsc_dex]; через вызываемые — index.*.
//!
//! Эндпоинты: GET /networks/{network}/tokens/{address}/pools?page=N&include=base_token,quote_token,dex
//! («Returns up to 20 pools per page», сортировка по умолчанию h24_volume_usd_liquidity_desc) и
//! GET /networks/{network}/dexes?page=N — https://docs.coingecko.com/demo/reference/top-pools-contract-address ,
//! https://docs.coingecko.com/demo/reference/endpoint-overview .

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use alloy_primitives::Address;
use chrono::Utc;
use tokio::sync::mpsc::UnboundedSender;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::settings::live::Watch;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::gt_http;
use super::{fetch, store};

/// Работает, пока жива подписка. Принимает: окружение, монеты (адрес → тикер), встречные токены (USDT и
/// промежуточные), канал «монета обновлена» (None — никто не слушает). Ошибок не возвращает: всё — в журнал и в
/// состояние справочника (интерфейс).
pub async fn run(
    ctx: Ctx,
    coins: HashMap<Address, String>,
    quotes: HashSet<Address>,
    tx: Option<UnboundedSender<Address>>,
) {
    prepare(&ctx, &coins, quotes);
    let mut watch = Watch::new(ctx.settings.subscribe(), vec!["index.".into()]);
    let mut loaded: Option<String> = None;
    loop {
        let wait = match pass(&ctx, &coins, &tx, &mut loaded).await {
            Ok(w) => {
                ctx.index.with(|s| s.error = None);
                Some(w)
            }
            Err(e) => {
                let changed = ctx.index.with(|s| {
                    let was = s.error.replace(e.message.clone());
                    was.as_deref() != Some(e.message.as_str())
                });
                if changed {
                    let msg = format!("Справочник пулов не обновляется: {}", e.message);
                    ctx.log(Level::Warn, "dex.index", msg, None).await;
                }
                // Секреты разблокируют не настройкой — повтор через index.refresh_s, если он задан.
                ctx.int("index.refresh_s").ok().map(Duration::from_secs)
            }
        };
        // Ждём срока следующей монеты (или повтора после ошибки) или изменения index.* (без срока — только изменения).
        let sleep = async {
            match wait {
                Some(w) => tokio::time::sleep(w).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = sleep => {}
            c = watch.next() => if c.is_none() { return },
        }
    }
}

/// Один проход: список DEX и монеты, у которых подошёл срок. Возвращает: через сколько следующий срок.
/// Ошибки: NotConfigured (index.*), SecretsLocked — проход не начат или прерван.
async fn pass(
    ctx: &Ctx,
    coins: &HashMap<Address, String>,
    tx: &Option<UnboundedSender<Address>>,
    loaded: &mut Option<String>,
) -> GatewayResult<Duration> {
    let (_, net) = gt_http::base(ctx)?;
    let every = chrono::Duration::seconds(ctx.int("index.refresh_s")? as i64);
    let pages = ctx.int("index.pages_per_token")? as u32;
    let size = ctx.int("index.page_size")? as usize;
    if loaded.as_deref() != Some(net.as_str()) {
        restore(ctx, &net, coins).await?;
        *loaded = Some(net.clone());
    }
    let now = Utc::now();
    if ctx.index.with(|s| {
        s.dexes_tried
            .or(s.dexes_at)
            .is_none_or(|at| at + every <= now)
    }) {
        ctx.index.with(|s| s.dexes_tried = Some(now));
        fetch::dexes(ctx, &net).await?;
    }
    let mut due: Vec<(Option<chrono::DateTime<Utc>>, Address)> = ctx.index.with(|s| {
        s.tokens
            .iter()
            .filter(|(_, t)| {
                t.tried_at
                    .or(t.fetched_at)
                    .is_none_or(|at| at + every <= now)
            })
            .map(|(a, t)| (t.tried_at.or(t.fetched_at), *a))
            .collect()
    });
    due.sort();
    let (mut ok, mut failed, mut first_err) = (0, 0, None);
    for (_, token) in due {
        ctx.index
            .with(|s| s.tokens.entry(token).or_default().tried_at = Some(Utc::now()));
        match fetch::token_pools(ctx, &net, token, pages, size).await {
            Ok((pools, n)) => {
                ok += 1;
                let entry = ctx.index.with(|s| {
                    let t = s.tokens.entry(token).or_default();
                    (t.fetched_at, t.pages, t.pools, t.error) = (Some(Utc::now()), n, pools, None);
                    t.clone()
                });
                if let Err(e) = store::save_token(ctx, &net, token, &entry).await {
                    ctx.log(Level::Error, "dex.index", e.message, None).await;
                }
                if let Some(tx) = tx {
                    let _ = tx.send(token);
                }
            }
            Err(e) if stops(&e) => return Err(e),
            Err(e) => {
                failed += 1;
                first_err.get_or_insert_with(|| e.message.clone());
                ctx.index
                    .with(|s| s.tokens.entry(token).or_default().error = Some(e.message));
            }
        }
    }
    if ok + failed > 0 {
        let tail = first_err
            .map(|e| format!(" (первая: {e})"))
            .unwrap_or_default();
        let msg = format!("Справочник пулов: спрошено монет {ok}, ошибок {failed}{tail}");
        ctx.log(Level::Info, "dex.index", msg, None).await;
    }
    Ok(next_due(ctx, every))
}

/// Ошибка, при которой проход не продолжается (настройка, секреты): остальные монеты дадут то же. Ошибок нет.
pub fn stops(e: &GatewayError) -> bool {
    matches!(e.code, ErrorCode::NotConfigured | ErrorCode::SecretsLocked)
}

/// Через сколько подойдёт срок ближайшей монеты или списка DEX (не меньше нуля). Ошибок нет.
fn next_due(ctx: &Ctx, every: chrono::Duration) -> Duration {
    let now = Utc::now();
    let soonest = ctx.index.with(|s| {
        s.tokens
            .values()
            .map(|t| t.tried_at.or(t.fetched_at).map_or(now, |at| at + every))
            .chain(std::iter::once(
                s.dexes_tried.or(s.dexes_at).map_or(now, |at| at + every),
            ))
            .min()
    });
    soonest
        .and_then(|t| (t - now).to_std().ok())
        .unwrap_or(Duration::ZERO)
}

/// Монеты и встречные токены подписки — в состояние справочника (лишние монеты убираются). Ошибок нет.
pub fn prepare(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: HashSet<Address>) {
    ctx.index.with(|s| {
        s.quotes = quotes;
        s.tokens.retain(|a, _| coins.contains_key(a));
        for (a, t) in coins {
            s.tokens.entry(*a).or_default().ticker = t.clone();
        }
    });
}

/// Сохранённое в SQLite — в память (только монеты подписки). Ошибки: Internal.
pub async fn restore(ctx: &Ctx, net: &str, coins: &HashMap<Address, String>) -> GatewayResult<()> {
    let tokens = store::load_tokens(ctx, net).await?;
    let dexes = store::load_dexes(ctx, net).await?;
    ctx.index.with(|s| {
        s.tokens.retain(|a, _| coins.contains_key(a));
        for (a, mut t) in tokens.into_iter().filter(|(a, _)| coins.contains_key(a)) {
            t.ticker = coins[&a].clone();
            s.tokens.insert(a, t);
        }
        if let Some((at, list)) = dexes {
            (s.dexes_at, s.dexes) = (Some(at), list);
        }
    });
    Ok(())
}
