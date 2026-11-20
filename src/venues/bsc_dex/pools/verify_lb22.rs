//! Файл: venues/bsc_dex/pools/verify_lb22.rs
//! Что делает: решение 446 — проверка пула Liquidity Book V2.2 из справочника на своём узле. Сначала — всё, что у V2.1
//! (verify_lb.rs: getFactory = dex.factory, токены X / Y — монета и встречный, реестр фабрики getLBPairInformation(X, Y,
//! binStep) = этот адрес; сигнатуры у V2.2 те же — ILBFactory.sol L46-L51, L104-L107). Затем третьим проходом у
//! принятых пулов implementation() — из dex.lb_implementations: фабрика может сменить реализацию для новых пулов
//! (setLBPairImplementation), а клон пула навсегда исполняет свою; математика вида сверена с исходником — значит, берутся
//! только пулы реализаций, чей верифицированный код сверен с этим исходником (адреса задаёт Тигран).
//! Ревью 446 (С1): крючки здесь не проверяются — их меняет фабрика в любой момент, поэтому они проверяются при каждом
//! полном чтении пула (lb_hooks.rs): недопущенный крючок исключает обмен через пул с причиной, а не отклоняет пул.
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: verify_lb.rs, abi_lb22.rs, node/call.rs (try_chunks), ctx.rs, verify.rs.
//! Настройки: dex.factory, dex.lb_implementations [DEX вида lb_v22].
//!
//! Источник: lfj-gg/joe-v2@v2.2.0 (1297c38) — src/LBPair.sol L60, L81 (implementation — адрес реализации, immutable),
//! src/LBFactory.sol L307-L327 (setLBPairImplementation), L330-L380 (createLBPair:
//! клон ImmutableClone.cloneDeterministic, L365 — адрес реализации в коде клона; смена реализации старые пулы не
//! трогает).

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_lb22::ILBPair22 as P22;
use super::families::Family;
use super::verify::{Candidate, Verdict};
use super::verify_lb;

/// Вызовов третьего прохода на пул (implementation).
pub const THIRD: usize = 1;

/// Проверка пачки кандидатов V2.2. Возвращает: итог по каждому. Ошибок нет (отказ — с причиной; сбой узла — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut out = verify_lb::verify(ctx, cands, size).await;
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    let mut idx: Vec<usize> = Vec::new();
    for (i, v) in out.iter().enumerate() {
        if let Verdict::Taken(f) = v
            && let PoolRef::Contract(a) = f.reference
        {
            calls.extend(third_calls(a));
            idx.push(i);
        }
    }
    let got = call::try_chunks(ctx, &calls, "latest", size).await;
    for (i, part) in idx.into_iter().zip(got.chunks(THIRD)) {
        let answers: Result<Vec<Result<Bytes, String>>, String> = part.iter().cloned().collect();
        let bad = match answers {
            Err(node) => Some(Verdict::Failed(node)),
            Ok(a) if a.len() != THIRD => Some(Verdict::Failed("нет ответа узла".into())),
            Ok(a) => settings(ctx, cands[i].family)
                .and_then(|imp| third_pass(&a, &imp))
                .err()
                .map(Verdict::Rejected),
        };
        if let Some(v) = bad {
            out[i] = v;
        }
    }
    out
}

/// Вызовы третьего прохода к пулу: implementation. Ошибок нет.
pub fn third_calls(pool: Address) -> Vec<(Address, Bytes)> {
    vec![(pool, Bytes::from(P22::implementationCall {}.abi_encode()))]
}

/// Реализации третьего прохода (обязательна — без неё DEX не подключена, families.rs required). Ошибка — не задана
/// dex.lb_implementations или элемент не адрес.
fn settings(ctx: &Ctx, f: &Family) -> Result<Vec<Address>, String> {
    let list = ctx.dex_list("dex.lb_implementations", f.id).map_err(|e| e.message)?;
    list.iter()
        .map(|s| {
            Address::from_str(s.trim())
                .map_err(|_| format!("dex.lb_implementations: «{s}» — не адрес"))
        })
        .collect::<Result<Vec<_>, _>>()
}

/// Третий проход (без сети; его же зовут тесты на записанных ответах). Принимает: ответы по порядку third_calls,
/// допущенные реализации. Возвращает: Ok — пул берётся; Err — причина отказа.
pub fn third_pass(a: &[Result<Bytes, String>], implementations: &[Address]) -> Result<(), String> {
    let at = |i: usize, what: &str| match a.get(i) {
        Some(Ok(b)) => Ok(b.clone()),
        Some(Err(r)) => Err(format!("пул откатил {what}: {r} — не пул Liquidity Book V2.2")),
        None => Err(format!("нет ответа пула на {what}")),
    };
    let imp = P22::implementationCall::abi_decode_returns(&at(0, "implementation()")?)
        .map_err(|e| format!("implementation() не по ABI: {e}"))?;
    if !implementations.contains(&imp) {
        return Err(format!(
            "реализация пула {imp:#x} не в dex.lb_implementations (код не сверен с исходником вида)"
        ));
    }
    Ok(())
}
