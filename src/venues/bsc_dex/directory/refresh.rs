//! Файл: venues/bsc_dex/directory/refresh.rs
//! Что делает: обновление справочника пулов от индексатора (решение 58) — задача подписки на рынок. По каждой монете
//! подписки (и промежуточной монете) — пулы монеты страницами (не больше index.pages_per_token; страница короче
//! index.page_size — последняя), не чаще раза в index.refresh_s; список DEX сети — так же раз в index.refresh_s,
//! по links.next. После перезапуска — из SQLite (store.rs): свежие монеты не спрашиваются. Каждую обновлённую монету
//! получает слушатель (канал `tx`) — проверка на своём узле и подключение пулов. Ошибка запроса монеты не стирает
//! прежний ответ (пол-списка — не факт): видна в интерфейсе, монета спрашивается снова через index.error_retry_s
//! (решение 61; не задана — через index.refresh_s). Сырой ответ любого кода (и 404) — в сыром журнале (gt_http.rs).
//! 404 по монете — не ошибка, а сведение «индексатор не знает пулов монеты» (решение 121, C2): пулов у индексатора
//! нет, монета спрашивается снова через index.refresh_s, в журнал — Info один раз на переход.
//! После перезапуска восстанавливаются и пулы поиска по фабрикам (store_factory.rs, ревью 28.11.2026, Б4).
//! С2 (ревью 28.11.2026): конец прохода и отправленные монеты — в счёт первого прохода справочника (boot.rs).
//! Пачка изменений index.* — один проход (решение 279: Watch::next_batch).
//! Решение 449: разблокировка или изменение секретов (secrets::vault::changes) — тоже проход сразу (раньше после ошибки
//! «Секреты не разблокированы» следующий проход был только через index.refresh_s, у Тиграна — 30 ч, или по изменению
//! index.*); проход, остановленный этой ошибкой, не считает список DEX и монету спрошенными — запроса не было.
//! Кто вызывает: pools/market.rs (задача подписки; при новой подписке прерывается).
//! От чего зависит: ctx.rs, directory/{fetch,gt_http,store,mod}.rs, settings/live.rs.
//! Настройки: index.refresh_s, index.error_retry_s, index.pages_per_token, index.page_size [bsc_dex]; через вызываемые — index.*.
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

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::settings::live::Watch;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::gt_http;
use super::{due, fetch, store};

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
    let mut secrets = crate::secrets::vault::changes();
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
                // Повтор через index.refresh_s (если задан); раньше — по изменению index.* или
                // секретов (решение 449).
                ctx.int("index.refresh_s").ok().map(Duration::from_secs)
            }
        };
        // С2: проход индексатора окончен (с итогом или ошибкой) — первый проход справочника его больше не ждёт (boot.rs).
        super::boot::source_done(&ctx, super::boot::Source::Indexer);
        // Ждём срока следующей монеты (или повтора после ошибки), изменения index.* или
        // секретов (без срока — только изменения).
        let sleep = async {
            match wait {
                Some(w) => tokio::time::sleep(w).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = sleep => {}
            c = watch.next_batch() => if c.is_none() { return },
            // Отправитель рассылки живёт весь процесс: Err не бывает (и тогда — просто проход).
            _ = secrets.changed() => {}
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
    // Монета с ошибкой (404 и т.п.) — повтор через index.error_retry_s; не задана — через index.refresh_s.
    let err_every = match ctx.int("index.error_retry_s") {
        Ok(s) => chrono::Duration::seconds(s as i64),
        Err(e) if e.code == ErrorCode::NotConfigured => every,
        Err(e) => return Err(e),
    };
    let gap = |t: &super::TokenEntry| if t.error.is_some() { err_every } else { every };
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
        let was = ctx.index.with(|s| s.dexes_tried.replace(now));
        if let Err(e) = fetch::dexes(ctx, &net).await {
            // Решение 449: остановка (секреты, настройка) — запроса не было, срок DEX прежний.
            ctx.index.with(|s| s.dexes_tried = was);
            return Err(e);
        }
    }
    let mut due: Vec<(Option<chrono::DateTime<Utc>>, Address)> = ctx.index.with(|s| {
        s.tokens
            .iter()
            .filter(|(_, t)| {
                t.tried_at
                    .or(t.fetched_at)
                    .is_none_or(|at| at + gap(t) <= now)
            })
            .map(|(a, t)| (t.tried_at.or(t.fetched_at), *a))
            .collect()
    });
    due.sort();
    let (mut ok, mut failed, mut first_err) = (0, 0, None);
    for (_, token) in due {
        let at = Utc::now();
        let was = ctx
            .index
            .with(|s| s.tokens.entry(token).or_default().tried_at.replace(at));
        match fetch::token_pools(ctx, &net, token, pages, size).await {
            Ok((pools, n, note)) => {
                ok += 1;
                let (entry, fresh_note) = ctx.index.with(|s| {
                    let t = s.tokens.entry(token).or_default();
                    let fresh = note.is_some() && t.note.is_none();
                    (t.fetched_at, t.pages, t.pools, t.error) = (Some(Utc::now()), n, pools, None);
                    t.note = note;
                    (t.clone(), fresh)
                });
                // Решение 121 (C2): «индексатор не знает пулов монеты» — сведение Info, один раз на переход.
                if fresh_note {
                    let why = entry.note.clone().unwrap_or_default();
                    let msg = format!("Справочник пулов: {} ({token:#x}): {why}", entry.ticker);
                    ctx.log(Level::Info, "dex.index", msg, None).await;
                }
                if let Err(e) = store::save_token(ctx, &net, token, &entry).await {
                    ctx.log(Level::Error, "dex.index", e.message, None).await;
                }
                if let Some(tx) = tx {
                    // С2: со счётом первого прохода справочника (boot.rs).
                    super::boot::send(ctx, tx, token);
                }
            }
            Err(e) if due::stops(&e) => {
                // Решение 449: остановка (секреты, настройка) — запроса не было, срок прежний.
                ctx.index
                    .with(|s| s.tokens.entry(token).or_default().tried_at = was);
                return Err(e);
            }
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
    Ok(due::next_due(ctx, every, &gap))
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
    // Б4 (арбитр 28.11.2026): пулы, найденные фабриками до перезапуска, — в справочник до стартовой проверки на узле
    // (pools/index.rs begin → take): площадка с пустым справочником индексатора не стартует с нулём пулов.
    let n = super::store_factory::restore(ctx, coins).await?;
    if n > 0 {
        let msg = format!(
            "Справочник пулов: из SQLite восстановлено пулов поиска по фабрикам {n} — проверка на узле"
        );
        ctx.log(Level::Info, "dex.index", msg, None).await;
    }
    Ok(())
}
