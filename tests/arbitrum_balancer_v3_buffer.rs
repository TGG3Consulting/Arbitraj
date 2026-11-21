//! Файл: tests/arbitrum_balancer_v3_buffer.rs
//! Что проверяет: решение 474 — обмен Balancer V3 через буфер ERC4626 хранилища на РЕАЛЬНЫХ ответах своего узла
//! Arbitrum, записанных на одном блоке скриптом tools/arbitrum_record/balancer_v3_buffer.py (папки
//! tests/data/arbitrum_bv3_buffer_<дата>/):
//! 1. Чтение — вызовы и разбор бота (load_balancer_v3::calls / decode с опорной суммой превью из meta.json): монета пары
//!    — базовый токен обёртки пула, сторона идёт через буфер (VaultV3.via).
//! 2. Правило хранилища побитово: расчёт бота (balancer_math::amount_out → math/balancer_v3.rs, balancer_v3_buffer.rs)
//!    с точными превью обёртки на суммах обмена (опорная сумма = сумма превью, записанные previewDeposit / previewRedeem)
//!    и без предела остатка буфера (запрос хранилища его не проверяет, Vault.sol L1231, L1352) = ответу BatchRouter
//!    querySwapExactIn того же блока; откат роутера — расчёт None.
//! 3. Превью по одной точке (опорная сумма dex.bv3_buffer_preview_raw; сумма ≤ опорной) не больше точного превью
//!    обёртки и меньше его не больше чем на 1 единицу токена — граница для линейного превью (шапка
//!    math/balancer_v3_buffer.rs, «Точность»); у записанной обёртки с нелинейным превью тест упадёт — это факт для
//!    решения Тиграна, не ошибка расчёта хранилища.
//!
//! Пока записи нет — тест помечен #[ignore] (запуск после записи: cargo test --test arbitrum_balancer_v3_buffer --
//! --ignored; затем метку снять).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::{balancer_v3_factories, balancer_v3_hooks, load_balancer_v3,
//! load_balancer_v3_extra, load_balancer_v3_fixed}, arb::venues::bsc_dex::math::{balancer_math, balancer_v3_buffer},
//! serde_json. Моков нет.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::core::types::PoolState;
use arb::core::types::pool_vault::V3Buffer;
use arb::venues::bsc_dex::math::{balancer_math, balancer_v3_buffer};
use arb::venues::bsc_dex::pools::balancer_v3_factories::{LISTS, Release};
use arb::venues::bsc_dex::pools::balancer_v3_hooks::{HookKind, Lists};
use arb::venues::bsc_dex::pools::load_balancer_v3_fixed::{Fixed, first, first_calls};
use arb::venues::bsc_dex::pools::{load_balancer_v3, load_balancer_v3_extra as extra};

/// Папки записи (tests/data/arbitrum_bv3_buffer_*). Нет ни одной — паника с подсказкой.
fn dirs() -> Vec<PathBuf> {
    let base = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data"));
    let mut out: Vec<PathBuf> = std::fs::read_dir(&base)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_bv3_buffer_"))
        })
        .collect();
    out.sort();
    assert!(
        !out.is_empty(),
        "нет записи: tools/arbitrum_record/balancer_v3_buffer.py"
    );
    out
}

/// JSON-файл.
fn json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Ответ записанного вызова: Ok(байты) или Err(текст ошибки узла).
fn answer(v: &Value) -> Result<Bytes, String> {
    match v["result"].as_str() {
        Some(h) => Ok(h.parse().unwrap()),
        None => Err(v["error"].to_string()),
    }
}

/// Число из ответа: первое слово (uint256) или pathAmountsOut[0] ответа querySwapExactIn (смещение, длина, элемент).
fn word(b: &[u8], at: usize) -> Option<U256> {
    b.get(at * 32..at * 32 + 32).map(U256::from_be_slice)
}

/// pathAmountsOut[0] ответа BatchRouter. None — откат или пусто.
fn path_out(v: &Value) -> Option<U256> {
    let b = answer(v).ok()?;
    let off: usize = word(&b, 0)?.try_into().ok()?;
    word(&b, off / 32 + 1)
}

/// Точное превью обёртки из записи: (обёртка, вид, сумма) → ответ.
type Previews = HashMap<(Address, String, U256), Option<U256>>;

/// Записанные вызовы пула: (адрес, данные) → ответ.
type Calls = HashMap<(Address, Bytes), Result<Bytes, String>>;

/// Ответы на вызовы бота по записи; вызова нет в записи — Err (тест его не подменяет).
fn lookup(calls: &Calls, want: &[(Address, Bytes)]) -> Vec<Result<Bytes, String>> {
    want.iter()
        .map(|k| {
            calls
                .get(k)
                .cloned()
                .unwrap_or_else(|| Err(format!("нет в записи: {k:?}")))
        })
        .collect()
}

/// Неизменяемое пула теми же вызовами, что у бота, и опорная сумма превью. None — пул бот не берёт.
fn fixed_of(meta: &Value, pool: Address, calls: &Calls) -> Option<Fixed> {
    let vault: Address = meta["vault"].as_str().unwrap().parse().unwrap();
    let mut facs: Vec<(Address, Release)> = Vec::new();
    for (name, r) in LISTS {
        let l = meta["lists"][name].as_array().cloned().unwrap_or_default();
        facs.extend(l.iter().map(|a| (a.as_str().unwrap().parse().unwrap(), r)));
    }
    let hl = |n: &str| -> Vec<Address> {
        let l = meta["lists"][n].as_array().cloned().unwrap_or_default();
        l.iter()
            .map(|a| a.as_str().unwrap().parse().unwrap())
            .collect()
    };
    let hooks = Lists {
        surge: hl("dex.bv3_stable_surge_hooks"),
        mev: hl("dex.bv3_mev_capture_hooks"),
    };
    let got = first(
        &facs,
        &hooks,
        &lookup(calls, &first_calls(vault, pool, &facs)),
    )
    .ok()?;
    let (release, tokens, min_trade, hook) = got;
    let ex = extra::decode(release, &lookup(calls, &extra::calls(pool, release))).ok()?;
    let r = U256::from_str_radix(meta["preview_raw"].as_str().unwrap(), 10).unwrap();
    Some(Fixed {
        release,
        tokens,
        min_trade,
        hook,
        extra: ex,
        buffer: Some(r),
    })
}

/// Буфер с точным превью в одной точке x (опорная = x, превью = записанное) и без предела остатка.
fn exact(b: &V3Buffer, x: U256, deposit: Option<U256>, redeem: Option<U256>) -> V3Buffer {
    let mut e = b.clone();
    e.reference = x;
    e.deposit = deposit.unwrap_or(b.deposit);
    e.redeem = redeem.unwrap_or(b.redeem);
    e.underlying_bal = U256::MAX;
    e.wrapped_bal = U256::MAX;
    e
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/balancer_v3_buffer.py"]
fn buffer_quote_matches_batch_router() {
    let one = U256::from(1u8);
    let (mut same, mut near, mut skipped) = (0usize, 0usize, Vec::new());
    for dir in dirs() {
        let meta = json(&dir.join("meta.json"));
        let vault: Address = meta["vault"].as_str().unwrap().parse().unwrap();
        let sys: Address = meta["arb_sys"].as_str().unwrap().parse().unwrap();
        let owner: Address = meta["arb_owner_public"].as_str().unwrap().parse().unwrap();
        for e in std::fs::read_dir(&dir).unwrap() {
            let path = e.unwrap().path();
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            if !(name.starts_with("pool_") && name.ends_with(".json")) {
                continue;
            }
            let p = json(&path);
            let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
            let mut calls = HashMap::new();
            for c in p["calls"].as_array().unwrap() {
                let to: Address = c["to"].as_str().unwrap().parse().unwrap();
                let data: Bytes = c["data"].as_str().unwrap().parse().unwrap();
                calls.insert((to, data), answer(c));
            }
            let mut previews: Previews = HashMap::new();
            for v in p["previews"].as_array().unwrap() {
                let w: Address = v["wrapper"].as_str().unwrap().parse().unwrap();
                let x = U256::from_str_radix(v["amount"].as_str().unwrap(), 10).unwrap();
                let got = answer(v).ok().and_then(|b| word(&b, 0));
                previews.insert((w, v["kind"].as_str().unwrap().to_string(), x), got);
            }
            let Some(fx) = fixed_of(&meta, pool, &calls) else {
                skipped.push(format!("{pool:#x}: пул бот не берёт"));
                continue;
            };
            let pre = matches!(fx.hook, HookKind::Mev(_)).then_some((sys, owner));
            let want = load_balancer_v3::calls(vault, pool, &fx, pre);
            let answers = lookup(&calls, &want);
            for q in p["quotes"].as_array().unwrap() {
                let (i, j) = (
                    q["i"].as_u64().unwrap() as u8,
                    q["j"].as_u64().unwrap() as u8,
                );
                let t_in: Address = q["token_in"].as_str().unwrap().parse().unwrap();
                let t_out: Address = q["token_out"].as_str().unwrap().parse().unwrap();
                let amount = U256::from_str_radix(q["amount"].as_str().unwrap(), 10).unwrap();
                let st = match load_balancer_v3::decode(pool, &fx, &answers, (t_in, t_out)) {
                    Ok(PoolState::VaultPool(d)) => d,
                    Ok(_) => panic!("не VaultPool"),
                    Err(w) => {
                        skipped.push(format!("{pool:#x}: {w}"));
                        continue;
                    }
                };
                let v3 = st.v3.as_ref().unwrap();
                let via = |k: u8| v3.via.get(k as usize).cloned().flatten();
                let wi = q["wrap_in"].as_bool().unwrap();
                let wo = q["unwrap_out"].as_bool().unwrap();
                // Монета — сразу и токен пула, и базовый токен другой обёртки: бот берёт токен пула — не эта сверка.
                if (st.i0, st.i1) != (i, j) || via(i).is_some() != wi || via(j).is_some() != wo {
                    skipped.push(format!("{pool:#x} {i}→{j}: сторона пары — сам токен пула"));
                    continue;
                }
                let theirs = path_out(&q["answer"]);
                let mid = path_out(&q["pool_out"]);
                // 2. Точные превью на суммах обмена.
                let mut d = (*st).clone();
                let mut ok = true;
                if let Some(b) = via(i) {
                    let x = amount - one;
                    let key = (b.wrapped, "deposit".to_string(), x);
                    let dep = previews.get(&key).cloned().flatten();
                    ok &= dep.is_some();
                    // 3. Превью по одной точке — не дальше 1 единицы от точного.
                    if let Some(dep) = dep {
                        let free = exact(&b, b.reference, None, None);
                        let ours = balancer_v3_buffer::wrap(&free, amount);
                        if let Some(o) = ours.filter(|_| amount >= b.min_wrap) {
                            let t = dep - one;
                            assert!(o <= t && t - o <= one, "WRAP {pool:#x} {amount}: {o} / {t}");
                            near += 1;
                        }
                    }
                    let mut v = d.v3.take().unwrap();
                    v.via[i as usize] = Some(exact(&b, x, dep, None));
                    d.v3 = Some(v);
                }
                if let Some(b) = via(j) {
                    let x = mid.map(|m| m - one);
                    let key = x.map(|x| (b.wrapped, "redeem".to_string(), x));
                    let red = key.and_then(|k| previews.get(&k).cloned().flatten());
                    ok &= red.is_some() || mid.is_none();
                    if let (Some(x), Some(red)) = (x, red) {
                        let free = exact(&b, b.reference, None, None);
                        let ours = balancer_v3_buffer::unwrap(&free, x + one);
                        if let Some(o) = ours {
                            let t = red - one;
                            assert!(o <= t && t - o <= one, "UNWRAP {pool:#x} {x}: {o} / {t}");
                            near += 1;
                        }
                        let mut v = d.v3.take().unwrap();
                        v.via[j as usize] = Some(exact(&b, x, None, Some(red)));
                        d.v3 = Some(v);
                    }
                }
                if !ok {
                    let why = format!("{pool:#x} {i}→{j} {amount}: нет точного превью в записи");
                    skipped.push(why);
                    continue;
                }
                let ours = balancer_math::amount_out(&d, i.into(), j.into(), amount);
                assert_eq!(
                    ours, theirs,
                    "пул {pool:#x} {i}→{j} wrap {wi} unwrap {wo} {amount}"
                );
                same += 1;
            }
        }
    }
    assert!(same > 0, "ни одной сверки (не взяты: {skipped:?})");
    eprintln!(
        "сверок {same}, превью по одной точке {near}, пропущено {}",
        skipped.len()
    );
}
