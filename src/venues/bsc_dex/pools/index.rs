//! Файл: venues/bsc_dex/pools/index.rs
//! Что делает: пулы из справочника индексатора → пулы подписки (решение 58). По каждому пулу с USDT или промежуточной
//! монетой (и пулу из события создания, и пулу, найденному фабрикой на своём узле — factory_scan.rs, решение 121): DEX индексатора — по таблице сопоставления (families::classify: не подключена —
//! причина на экране), затем проверка на своём узле (verify.rs) — всех пулов с прочитанных страниц: ликвидность
//! индексатора ни в каком решении не участвует, мелкие пулы отсекает своя мера dex.min_pool_liquidity_usd в подписке.
//! Прошедшие — во «взятые» (их слушает подписка); не прошедшие — причина в справочнике и один раз в журнал; сбой узла —
//! «не проверен» (повтор — index_watch.rs). Пул, исключённый проверкой «кто меняет» (check/emitter.rs), не берётся.
//! Взятые отдаются подписке в порядке загрузки (order.rs, решение 61: по ликвидности индексатора — только порядок).
//! Решение 295: у DEX в режиме dex.multi_pair ключ строки (status / taken) — пул + пара (multi_pair.rs), после проверки —
//! развёртка недостающих пар взятых многомонетных пулов; режим не включён — ключ и проход прежние.
//! Проход проверки (режим, ключи, развёртка) — index_pass.rs.
//! Кто вызывает: pools/market.rs (begin), index_watch.rs, created_log.rs (record).
//! От чего зависит: ctx.rs, directory (состояние, refresh::prepare/restore), families.rs, verify.rs, discover.rs, mids.rs.
//! Настройки: index.network [bsc_dex]; через вызываемые — dex.verify_batch, dex.* [DEX].

use std::collections::{BTreeSet, HashMap, HashSet};

use alloy_primitives::Address;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;

use crate::core::types::DirectoryStatus;
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::directory::refresh;
use super::discover::{Found, against};
use super::mids::Mid;
use super::order;
use super::verify::{Candidate, Verdict};
use super::{factory_scan, index_pass, multi_pair};

/// Итог проверки: взятые пулы (запрошенных монет) и сколько не проверено из-за узла.
pub struct Took {
    pub found: Vec<Found>,
    pub failed: usize,
    /// Монеты строк, для которых проверка узла не дала вердикта (включая развёртку многомонетных пулов).
    pub retry_coins: HashSet<Address>,
}

/// Справочник для подписки: встречные токены (USDT и промежуточные), старт, задача обновления от индексатора
/// (directory/refresh.rs) и задача поиска по фабрикам на своём узле (factory_scan.rs, решение 121, C3) — обе шлют
/// в один канал «монета обновлена». Возвращает задачи, приёмник и очередь повторов начальной проверки. Ошибок нет.
pub async fn begin(
    ctx: &Ctx,
    all: &HashMap<Address, String>,
    usdt: Address,
    mids: &[Mid],
) -> (
    Vec<JoinHandle<()>>,
    UnboundedReceiver<Address>,
    HashSet<Address>,
) {
    let mut retry_coins = HashSet::new();
    let quotes: HashSet<Address> = std::iter::once(usdt)
        .chain(mids.iter().map(|m| m.address))
        .collect();
    refresh::prepare(ctx, all, quotes.clone());
    let res = match ctx.text("index.network") {
        Ok(net) => refresh::restore(ctx, &net, all).await,
        Err(e) => Err(e),
    };
    if let Err(e) = res {
        let msg = format!("Справочник пулов при старте подписки: {}", e.message);
        ctx.log(Level::Warn, "dex.index", msg, None).await;
    } else {
        match take(ctx, None).await {
            Ok(took) => retry_coins = took.retry_coins,
            Err(e) => {
                let msg = format!(
                    "Справочник пулов: проверка при старте не выполнена: {}",
                    e.message
                );
                ctx.log(Level::Warn, "dex.index", msg, None).await;
            }
        }
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let scan = factory_scan::run(ctx.clone(), all.clone(), quotes.clone(), tx.clone());
    let task = tokio::spawn(refresh::run(ctx.clone(), all.clone(), quotes, Some(tx)));
    (vec![task, tokio::spawn(scan)], rx, retry_coins)
}

/// Проверяет пулы справочника (монеты `only` или все) и обновляет итог по каждому. Решение 295: у DEX в режиме
/// dex.multi_pair ключ строки — пул + пара (multi_pair::dir_key), после проверки — развёртка недостающих пар взятых
/// многомонетных пулов и одна проверка новых строк в этом же вызове; режим нигде не включён — проход прежний.
/// Возвращает: взятые пулы этих монет (и монет развёртки) и число непроверенных из-за узла.
/// Ошибки: NotConfigured (dex.verify_batch).
pub async fn take(ctx: &Ctx, only: Option<&HashSet<Address>>) -> GatewayResult<Took> {
    index_pass::take_with(ctx, only, index_pass::on_node).await
}

/// Итоги проверки — в справочник: взят / отклонён (в журнал — один раз на пул и причину) / не проверен (узел).
/// Пул, исключённый проверкой «кто меняет», не берётся. Возвращает: сколько не проверено из-за узла. Ошибок нет.
pub async fn record(ctx: &Ctx, cands: &[Candidate], verdicts: Vec<Verdict>) -> usize {
    record_with(ctx, cands, None, verdicts).await
}

/// Что делает: record с заданными id строк справочника (решение 450: строка индексатора с dex.sub_dex проверяется
/// видом дочернего ключа, а ключ строки — по id индексатора). Принимает: окружение, кандидатов, id DEX строк по
/// порядку кандидатов (None — id вида кандидата, как раньше), итоги. Возвращает: сколько не проверено из-за узла.
/// Ошибок нет.
pub async fn record_with(
    ctx: &Ctx,
    cands: &[Candidate],
    ids: Option<&[String]>,
    verdicts: Vec<Verdict>,
) -> usize {
    let mut failed = 0;
    let mut log = Vec::new();
    ctx.index.with(|s| {
        for (n, (c, v)) in cands.iter().zip(verdicts).enumerate() {
            let id = ids
                .and_then(|l| l.get(n))
                .map_or(c.family.id, |x| x.as_str());
            // Решение 295: ключ строки — с парой у DEX в режиме dex.multi_pair, иначе строка пула (как до решения).
            let key = multi_pair::dir_key(&s.multi_dex, id, &c.pool, c.coin, c.quote);
            let st = match v {
                Verdict::Taken(f) => match s.banned.get(&f.reference.site()) {
                    Some(why) => DirectoryStatus::Rejected(why.clone()),
                    None => {
                        s.taken.insert(key.clone(), *f);
                        s.status.insert(key, DirectoryStatus::Taken);
                        continue;
                    }
                },
                Verdict::Rejected(why) => {
                    if s.logged.insert(format!("{} {why}", c.pool)) {
                        log.push(format!("{} {} ({}): {why}", c.family.id, c.pool, c.coin));
                    }
                    DirectoryStatus::Rejected(why)
                }
                Verdict::Failed(_) => {
                    failed += 1;
                    DirectoryStatus::Unchecked
                }
            };
            s.taken.remove(&key);
            s.status.insert(key, st);
        }
    });
    for r in log {
        ctx.log(
            Level::Warn,
            "dex.index",
            format!("Пул из справочника отклонён: {r}"),
            None,
        )
        .await;
    }
    failed
}

/// Одна запись журнала на смену списка DEX с пулами монет, но не подключённых. Ошибок нет.
pub(super) async fn summary(ctx: &Ctx, now: BTreeSet<String>) {
    let changed = ctx.index.with(|s| {
        let c = s.unconnected != now;
        s.unconnected = now.clone();
        c
    });
    if changed && !now.is_empty() {
        let list: Vec<String> = now.into_iter().collect();
        let msg = format!(
            "Справочник: DEX с пулами монет, но не подключены — {}",
            list.join(", ")
        );
        ctx.log(Level::Info, "dex.index", msg, None).await;
    }
}

/// Взятые пулы со встречным токеном (монета — другая валюта пула и есть в списке) — в порядке загрузки
/// (pools/order.rs). Ошибок нет.
pub fn taken_against(ctx: &Ctx, coins: &HashMap<Address, String>, quote: Address) -> Vec<Found> {
    let all: Vec<Found> = ctx.index.with(|s| s.taken.values().cloned().collect());
    let mine = all
        .iter()
        .filter_map(|f| against(f, quote))
        .filter(|f| f.coin != quote && coins.contains_key(&f.coin))
        .collect();
    order::order(ctx, mine)
}
