//! Файл: venues/bsc_dex/pools/multi_pair.rs
//! Что делает: решение 295 — все нужные боту пары многомонетного пула (Curve 2020 / Ellipsis, Curve StableSwap-NG,
//! Balancer V2, Saddle / Nerve, PancakeSwap StableSwap) как отдельные рёбра. Режим — настройка dex.multi_pair [DEX]:
//! да — проверка пула этих видов строит ссылку `PoolRef::Multi { контракт, пара }` (одна запись реестра на пару) и
//! запоминает монеты пула для развёртки остальных пар; не задана / нет — одна ссылка `Contract` на пул, как до решения.
//! Ключ справочника (status / taken / seen): у DEX в режиме — «пул|монета_меньше|монета_больше», у остальных — строка
//! пула как есть (то есть у парных пулов ключ не меняется). Решение 305: WooFi (woofi_v2, общий контракт пар) с
//! dex.multi_pair = да — тоже ключ с парой (woo_pairs; ссылка пары — прежняя Member, развёртки нет).
//! Кто вызывает: pools/index.rs (take: режим, ключ, развёртка), verify_curve.rs, verify_curve_ng.rs, verify_balancer.rs,
//! verify_stable_u.rs (adopt), directory/view.rs, created_log.rs, factory_scan.rs (dir_key), market.rs (fits),
//! woo_quote.rs и factory_calls.rs (woo_pairs). Решение 474: пары «обёртка — её базовый токен» пула Balancer V3
//! (Snap.pool_twins, verify_balancer_v3_buffer.rs) в развёртке не строятся.
//! От чего зависит: ctx.rs, families.rs, discover.rs (Found), directory (Snap, IndexedPool), stable_list.rs (pairs).
//! Настройки: dex.multi_pair, dex.sub_dex [DEX].

use std::collections::HashSet;

use alloy_primitives::Address;

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::directory::Snap;
use super::super::directory::gt_parse::IndexedPool;
use super::discover::{Found, sorted};
use super::families::{self, Family, Proto};
use super::registry::Entry;
use super::stable_list::pairs;

/// Что делает: включён ли режим «все пары пула» у DEX этого вида. Принимает: окружение, вид DEX.
/// Возвращает: true — вид многомонетный и dex.multi_pair = да; иначе (не задана, нет, не читается) — false. Ошибок нет.
///
/// Ревью 450 (п. 1): флаг — групповой (flag): у ключа или у любого ключа его группы dex.sub_dex — иначе ссылка пары
/// (adopt), сверка реестра (entry_fits) и ключ справочника (multi_dex) расходились бы, пары одного контракта
/// перетирали бы друг друга в реестре, а retune снимал бы их каждый проход.
pub fn enabled(ctx: &Ctx, f: &Family) -> bool {
    f.proto.is_multi_coin() && flag(ctx, f.id)
}

/// Ревью 450 (п. 1): dex.multi_pair = да у ключа или у любого ключа его группы dex.sub_dex (sub_dex::group). Сначала —
/// свой флаг (без группы — как до решения 450). Принимает: окружение, ключ. Возвращает: true / false. Ошибок нет.
fn flag(ctx: &Ctx, id: &str) -> bool {
    let on = |k: &str| matches!(ctx.dex_flag("dex.multi_pair", k), Ok(true));
    on(id) || super::sub_dex::group(ctx, id).iter().any(|k| on(k))
}

/// Что делает: решение 305 — строки справочника WooFi (вид woofi_v2) с ключом-ПАРОЙ: dex.multi_pair = да у этой DEX.
/// WooPPV2 — один общий контракт на все пары (PoolRef::Member), ссылка пары не меняется (развёртки и Multi нет); меняется
/// только ключ строки справочника (status / taken / seen). Принимает: окружение, вид DEX. Возвращает: true — вид
/// woofi_v2 и dex.multi_pair = да; иначе (не задана, нет, не читается) — false, как до решения. Ошибок нет.
/// Ревью 450 (п. 1): флаг — групповой, как у enabled.
pub fn woo_pairs(ctx: &Ctx, f: &Family) -> bool {
    f.proto == Proto::WooPP && flag(ctx, f.id)
}

/// Что делает: id DEX, у которых строка справочника — ПАРА (ключ «пул|монета|монета», dir_key): DEX в режиме «все пары
/// пула» и — решение 305 — DEX вида woofi_v2 с dex.multi_pair = да (woo_pairs: с ключом-строкой пула все пары WooFi
/// одного контракта сливались в одну запись seen / status / taken — проверялась и бралась одна пара на весь контракт).
/// Решение 450: ключ с dex.sub_dex и его дочерние ключи — в множестве вместе, если в нём хоть один из них (строка
/// индексатора и строки развёртки дочернего вида получают один и тот же ключ — без дублей пары).
/// Принимает: окружение. Возвращает: множество id (пусто — справочник и отбор работают как до решения 295). Ошибок нет.
pub fn multi_dex(ctx: &Ctx) -> HashSet<String> {
    let mut out: HashSet<String> = families::all(ctx)
        .into_iter()
        .filter(|f| enabled(ctx, f) || woo_pairs(ctx, f))
        .map(|f| f.id.to_string())
        .collect();
    for (parent, subs) in super::sub_dex::groups(ctx) {
        if out.contains(&parent) || subs.iter().any(|k| out.contains(k)) {
            out.insert(parent);
            out.extend(subs);
        }
    }
    out
}

/// Что делает: ключ строки справочника. Принимает: DEX в режиме, id DEX, строку пула, две монеты пары.
/// Возвращает: строку пула как есть (DEX не в режиме — так у всех парных пулов) или «пул|меньшая|большая». Ошибок нет.
pub fn dir_key(multi: &HashSet<String>, dex_id: &str, pool: &str, a: Address, b: Address) -> String {
    if !multi.contains(dex_id) {
        return pool.to_string();
    }
    let (lo, hi) = sorted(a, b);
    format!("{pool}|{lo:#x}|{hi:#x}")
}

/// Что делает: найденный пул многомонетного вида в режиме — ссылка Multi с парой и монеты пула в справочник (для
/// развёртки). Вне режима — пул без изменений. Принимает: окружение, найденный пул (ссылка Contract), монеты пула.
/// Возвращает: пул. Ошибок нет.
pub fn adopt(ctx: &Ctx, mut f: Found, pool_tokens: &[Address]) -> Found {
    let PoolRef::Contract(contract) = f.reference else {
        return f;
    };
    if !enabled(ctx, f.family) {
        return f;
    }
    f.reference = PoolRef::Multi {
        contract,
        token0: f.token0,
        token1: f.token1,
    };
    let key = format!("{contract:#x}");
    ctx.index
        .with(|s| s.pool_tokens.insert(key, pool_tokens.to_vec()));
    f
}

/// Что делает: совпадает ли вид ссылки пула с режимом его DEX (смена dex.multi_pair на лету). Принимает: вид DEX,
/// ссылку, DEX в режиме. Возвращает: true — у немногомонетного вида всегда; у многомонетного — Multi ⇔ DEX в режиме.
/// Ошибок нет.
pub fn fits(f: &Family, r: &PoolRef, multi: &HashSet<String>) -> bool {
    !f.proto.is_multi_coin() || matches!(r, PoolRef::Multi { .. }) == multi.contains(f.id)
}

/// Что делает: запись реестра подходит под режим (market.rs, retain при пересборке подписки). Принимает: окружение,
/// запись, ссылки пулов, найденных для этой подписки (взятые справочника после retune и пары через промежуточные).
/// Возвращает: многомонетный вид — Multi ⇔ DEX в режиме (как fits); решение 305 (ревью В1): WooFi — только если её
/// пара среди найденных (ссылка Member в обоих режимах dex.multi_pair одна, поэтому вид ссылки режим не выдаёт: после
/// «да → нет» retune снимает взятые пары с ключом-парой, и запись реестра без этой сверки осталась бы без слушателя —
/// пачки слушания строятся только из найденного); прочие виды — true. Ошибок нет.
pub fn entry_fits(ctx: &Ctx, e: &Entry, known: &HashSet<PoolRef>) -> bool {
    if e.family.proto == Proto::WooPP {
        return known.contains(&e.pool.reference);
    }
    !e.family.proto.is_multi_coin() || matches!(e.pool.reference, PoolRef::Multi { .. }) == enabled(ctx, e.family)
}

/// Что делает: режим справочника на этот проход — запомнить DEX в режиме и снять «взятые», чья ссылка не совпадает с
/// режимом (их статусы тоже). Решение 305: у WooFi ссылка в обоих режимах одна (Member) — снимаются «взятые», чей ВИД
/// КЛЮЧА не совпадает с режимом (ключ-пара содержит «|», ключ-строка пула — нет; иначе после смены dex.multi_pair одна
/// пара осталась бы взятой дважды). Принимает: состояние справочника, DEX в режиме. Возвращает: ничего. Ошибок нет.
pub fn retune(s: &mut Snap, multi: &HashSet<String>) {
    s.multi_dex = multi.clone();
    let gone: Vec<String> = s
        .taken
        .iter()
        .filter(|(k, f)| {
            !fits(f.family, &f.reference, multi)
                || (f.family.proto == Proto::WooPP && k.contains('|') != multi.contains(f.family.id))
        })
        .map(|(k, _)| k.clone())
        .collect();
    for k in gone {
        s.taken.remove(&k);
        s.status.remove(&k);
    }
    // Строки развёртки DEX, вышедших из режима, — прочь (решение 305: в множестве может быть WooFi, у которой развёртки
    // нет, — поэтому не «очистить всё при пустом множестве», а по DEX; при пустом множестве — то же, что раньше).
    for list in s.multi.values_mut() {
        list.retain(|p| multi.contains(&p.dex_id));
    }
    s.multi.retain(|_, l| !l.is_empty());
}

/// Что делает: развёртка — по каждому взятому пулу с ссылкой Multi строит недостающие пары «монета справочника —
/// встречный» из монет пула (запомненных проверкой) и кладёт их строками справочника (источник multi). Пары — тем же
/// правилом, что у пулов из dex.stable_pools (stable_list::pairs). Принимает: состояние справочника.
/// Возвращает: ключи новых строк (пусто — развёртывать нечего). Ошибок нет.
pub fn expand(s: &mut Snap) -> HashSet<String> {
    // Взятых пар Multi нет (режим нигде не включён или пулов этих видов нет) — разворачивать нечего.
    if !s.taken.values().any(|f| matches!(f.reference, PoolRef::Multi { .. })) {
        return HashSet::new();
    }
    let mut coins: Vec<Address> = s.tokens.keys().copied().collect();
    coins.sort();
    let mut quotes: Vec<Address> = s.quotes.iter().copied().collect();
    quotes.sort();
    let mut have: HashSet<String> = HashSet::new();
    for (token, list) in s
        .tokens
        .iter()
        .map(|(a, t)| (a, t.pools.as_slice()))
        .chain(s.events.iter().map(|(a, l)| (a, l.as_slice())))
        .chain(s.factory.iter().map(|(a, l)| (a, l.as_slice())))
        .chain(s.multi.iter().map(|(a, l)| (a, l.as_slice())))
    {
        for p in list {
            let other = if p.base == *token { p.quote } else { p.base };
            have.insert(dir_key(&s.multi_dex, &p.dex_id, &p.pool, *token, other));
        }
    }
    let mut taken: Vec<(String, String)> = s
        .taken
        .values()
        .filter_map(|f| match f.reference {
            PoolRef::Multi { contract, .. } => Some((f.family.id.to_string(), format!("{contract:#x}"))),
            _ => None,
        })
        .collect();
    taken.sort();
    taken.dedup();
    let sym = |s: &Snap, a: Address| {
        s.tokens
            .get(&a)
            .map(|t| t.ticker.clone())
            .unwrap_or_else(|| format!("{a:#x}"))
    };
    let mut added = HashSet::new();
    for (dex, pool) in taken {
        let Some(tokens) = s.pool_tokens.get(&pool).cloned() else {
            continue;
        };
        let twins = s.pool_twins.get(&pool).cloned().unwrap_or_default();
        for (coin, quote) in pairs(&tokens, &coins, &quotes) {
            // Решение 474: обёртка ERC4626 и её базовый токен одного пула Balancer V3 — не пара.
            if super::verify_balancer_v3_buffer::twin(&twins, coin, quote) {
                continue;
            }
            let key = dir_key(&s.multi_dex, &dex, &pool, coin, quote);
            if !have.insert(key.clone()) {
                continue;
            }
            let row = IndexedPool {
                dex_id: dex.clone(),
                pool: pool.clone(),
                base: coin,
                quote,
                base_symbol: sym(s, coin),
                quote_symbol: sym(s, quote),
                reserve_usd: None,
            };
            s.multi.entry(coin).or_default().push(row);
            added.insert(key);
        }
    }
    added
}
