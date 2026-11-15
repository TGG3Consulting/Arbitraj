//! Файл: venues/bsc_dex/pools/stable_list.rs
//! Что делает: решение 292 — пулы dex.stable_pools видов saddle_stableswap (Nerve) и curve_stableswap (Ellipsis,
//! Curve 2020) — источник справочника при поиске по фабрикам (index.factory_scan = да), без индексатора: монеты пула
//! читаются у самого пула (getToken(k) / coins(k) до первого отката — load_saddle::tokens_of, load_curve::coins_of),
//! кандидат — каждая пара (монета подписки, встречный), обе монеты которой в пуле; дальше — index::take → проверка
//! (verify_stable_u.rs / verify_curve.rs) → реестр. Список не задан или пуст — ничего (как до 292). Одна пачка на проход.
//! Кто вызывает: pools/factory_scan.rs (pass); sort — ещё tests/bsc_dex_stable_list_sort.rs.
//! От чего зависит: ctx.rs, families.rs (all, classify, Proto), abi_saddle.rs, abi_curve.rs, load_saddle.rs,
//! load_curve.rs, node/call.rs (try_chunks), directory/gt_parse.rs (IndexedPool).
//! Настройки: dex.stable_pools [DEX] (через classify — dex.kind, dex.pool_enabled); dex.load_batch [bsc_dex] —
//! размер пачки передаёт factory_scan.rs.
//!
//! Источники ABI (официальные): Saddle — nerve-finance/contracts bda7578, Swap.sol L231 `getToken(uint8)` (откат
//! «Out of range» за последним индексом), не больше 32 монет — L125; Curve 2020 — ellipsis-finance/ellipsis 6bdf778,
//! contracts/StableSwap.vy L95 `coins: public(address[N_COINS])` (геттер coins(uint256), за N_COINS — откат).

use std::collections::HashMap;
use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use super::super::ctx::Ctx;
use super::super::directory::gt_parse::IndexedPool;
use super::super::node::call;
use super::abi_curve::{IStableSwapV1, MAX_COINS};
use super::abi_saddle::{ISaddleSwap, MAX_TOKENS};
use super::families::{self, Family, Proto};
use super::load_curve::coins_of;
use super::load_saddle::tokens_of;

/// Хвост заметки прохода без режима dex.multi_pair (решение 295 меняет его у DEX в режиме — scan).
const ONE_PAIR: &str = "в реестр попадёт одна пара пула (TASKS 289 п. 7)";

/// Итог источника за проход: найденные кандидаты по монете, заметки, вызовов без ответа узла, всего вызовов.
#[derive(Debug, Default)]
pub struct Extra {
    pub found: HashMap<Address, Vec<IndexedPool>>,
    pub notes: Vec<String>,
    pub no_answer: usize,
    pub calls: usize,
}

/// Вид, у которого список dex.stable_pools — источник пулов (решение 292). Ошибок нет.
pub fn is_listed_kind(proto: Proto) -> bool {
    matches!(proto, Proto::SaddleStable | Proto::CurveStable)
}

/// Вызовы «монеты пула»: Saddle — getToken(0..MAX_TOKENS), Curve 2020 — coins(0..MAX_COINS). Ошибок нет.
pub fn token_calls(proto: Proto, pool: Address) -> Vec<(Address, Bytes)> {
    match proto {
        Proto::SaddleStable => (0..MAX_TOKENS)
            .map(|k| {
                let data = ISaddleSwap::getTokenCall { index: k as u8 }.abi_encode();
                (pool, data.into())
            })
            .collect(),
        _ => (0..MAX_COINS)
            .map(|k| {
                let data = IStableSwapV1::coinsCall { i: U256::from(k) }.abi_encode();
                (pool, data.into())
            })
            .collect(),
    }
}

/// Монеты пула по ответам token_calls (до первого отката). Возвращает: адреса или текст «не по ABI». Ошибок нет.
pub fn tokens(proto: Proto, answers: &[Result<Bytes, String>]) -> Result<Vec<Address>, String> {
    match proto {
        Proto::SaddleStable => tokens_of(answers),
        _ => coins_of(answers),
    }
}

/// Пары (монета, встречный), обе монеты которых среди монет пула. Принимает: монеты пула, монеты подписки,
/// встречные токены. Возвращает: пары по порядку монет и встречных (монета ≠ встречный). Ошибок нет.
pub fn pairs(
    pool_tokens: &[Address],
    coins: &[Address],
    quotes: &[Address],
) -> Vec<(Address, Address)> {
    let mut out = Vec::new();
    for &coin in coins.iter().filter(|c| pool_tokens.contains(c)) {
        for &quote in quotes
            .iter()
            .filter(|q| **q != coin && pool_tokens.contains(q))
        {
            out.push((coin, quote));
        }
    }
    out
}

/// Кандидаты справочника по пулу — как у поиска по фабрике (factory_scan.rs sort). Принимает: id DEX, пул, монеты
/// пула, монеты подписки (адрес → тикер), встречные. Возвращает: записи справочника. Ошибок нет.
pub fn candidates(
    dex_id: &str,
    pool: Address,
    pool_tokens: &[Address],
    coins: &HashMap<Address, String>,
    quotes: &[Address],
) -> Vec<IndexedPool> {
    let sym = |a: Address| coins.get(&a).cloned().unwrap_or_else(|| format!("{a:#x}"));
    let mut coin_list: Vec<Address> = coins.keys().copied().collect();
    coin_list.sort();
    pairs(pool_tokens, &coin_list, quotes)
        .into_iter()
        .map(|(coin, quote)| IndexedPool {
            dex_id: dex_id.to_string(),
            pool: format!("{pool:#x}"),
            base: coin,
            quote,
            base_symbol: sym(coin),
            quote_symbol: sym(quote),
            reserve_usd: None,
        })
        .collect()
}

/// Пулы списков dex.stable_pools подключённых DEX этих видов. Возвращает: (DEX, пул) по порядку и заметки.
/// Список не задан или пуст — DEX пропускается молча (как до решения 292). Ошибок нет.
fn listed(ctx: &Ctx) -> (Vec<(&'static Family, Address)>, Vec<String>) {
    let (mut out, mut notes) = (Vec::new(), Vec::new());
    for f in families::all(ctx)
        .into_iter()
        .filter(|f| is_listed_kind(f.proto))
    {
        let list = match ctx.dex_list("dex.stable_pools", f.id) {
            Ok(l) if !l.is_empty() => l,
            _ => continue,
        };
        if let Err(why) = families::classify(ctx, f.id) {
            notes.push(format!(
                "{}: dex.stable_pools не опрашивается — {why}",
                f.id
            ));
            continue;
        }
        for s in &list {
            match Address::from_str(s.trim()) {
                Ok(pool) => out.push((f, pool)),
                Err(_) => notes.push(format!("{}: dex.stable_pools: «{s}» — не адрес", f.id)),
            }
        }
    }
    (out, notes)
}

/// Проход источника. Принимает: окружение, монеты подписки, встречные, размер пачки (dex.load_batch).
/// Возвращает: Extra. Ошибок нет (узел не ответил — no_answer, повтор решает factory_scan.rs).
pub async fn scan(
    ctx: &Ctx,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    size: usize,
) -> Extra {
    let (pools, notes) = listed(ctx);
    let calls: Vec<(Address, Bytes)> = pools
        .iter()
        .flat_map(|(f, pool)| token_calls(f.proto, *pool))
        .collect();
    if calls.is_empty() {
        return Extra {
            notes,
            ..Extra::default()
        };
    }
    let answers = call::try_chunks(ctx, &calls, "latest", size).await;
    let mut out = sort(&pools, answers, coins, quotes);
    // Решение 295: у DEX в режиме dex.multi_pair в реестр идут все пары пула — заметка прохода об этом.
    for f in pools.iter().map(|(f, _)| *f).filter(|f| super::multi_pair::enabled(ctx, f)) {
        let mine = format!("{}: пул ", f.id);
        for n in out.notes.iter_mut().filter(|n| n.starts_with(&mine)) {
            *n = n.replace(ONE_PAIR, "в реестр — все пары пула (dex.multi_pair, решение 295)");
        }
    }
    out.notes.splice(0..0, notes);
    out.calls = calls.len();
    out
}

/// Разбор ответов пачки (без сети): по каждому пулу — ровно его token_calls ответов (вычитываются целиком, даже если
/// среди них сбой узла — иначе следующий пул взял бы чужие ответы; ревью 292). Принимает: пулы по порядку вызовов,
/// ответы try_chunks, монеты подписки, встречные. Возвращает: Extra (calls = 0 — считает scan). Ошибок нет.
pub fn sort(
    pools: &[(&'static Family, Address)],
    answers: Vec<call::Piece>,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
) -> Extra {
    let mut x = Extra::default();
    let mut got = answers.into_iter();
    for (f, pool) in pools {
        let n = token_calls(f.proto, *pool).len();
        let part: Vec<call::Piece> = got.by_ref().take(n).collect();
        let Ok(answers) = part.into_iter().collect::<Result<Vec<_>, String>>() else {
            x.no_answer += n;
            continue;
        };
        let list = match tokens(f.proto, &answers) {
            Ok(t) => candidates(f.id, *pool, &t, coins, quotes),
            Err(why) => {
                x.notes.push(format!(
                    "{}: пул {pool:#x} из dex.stable_pools: {why}",
                    f.id
                ));
                continue;
            }
        };
        // Без dex.multi_pair: factory_scan.rs берёт одну запись на адрес пула внутри монеты, реестр — одну на пул
        // (TASKS 289 п. 7); в режиме — запись на пару (решение 295).
        x.notes.push(format!(
            "{}: пул {pool:#x} из dex.stable_pools — пар построено {}, {ONE_PAIR}",
            f.id,
            list.len()
        ));
        for p in list {
            x.found.entry(p.base).or_default().push(p);
        }
    }
    x
}
