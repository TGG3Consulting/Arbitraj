//! Файл: venues/bsc_dex/check/pass_calls.rs
//! Что делает: решение 401 (t186) — вызовы пробы шага пула менеджера через исполнитель кошелька (вынесено из
//! check/pass_probe.rs: файл был больше 200 строк): вызов execute одним шагом пула (форма шага без правила допуска —
//! exec/executor/entry.rs singleton_step), разрешения исполнителю, хвост стороны (квотер DEX, execute, balanceOf
//! исполнителя по токену входа и выхода), сторона для разбора (check/pass_judge.rs Side), откат подготовки — помеха.
//! Кто вызывает: check/pass_probe.rs.
//! От чего зависит: exec/executor/{call, entry, kind}.rs, quote/probe_quote.rs (Way), wallet/approve_executor.rs,
//! check/{simulate, probe_calls, pass_judge}.rs, pools/registry.rs.
//! Настройки: dex.deadline_s [bsc_dex]; через вызываемые — dex.vault [DEX Infinity], адрес квотера [DEX].

use alloy_primitives::{Address, U256};

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::ctx::Ctx;
use super::super::exec::executor::call::ExecCall;
use super::super::exec::executor::entry;
use super::super::exec::executor::kind::{self, ALL_BPS, StepSpec};
use super::super::pools::exec_kind::ExecKind;
use super::super::pools::registry::Entry;
use super::super::quote::probe_quote::Way;
use super::super::wallet::approve_executor;
use super::pass_judge::Side;
use super::probe_calls as pc;
use super::simulate::{Call, CallResult};

/// Номера вызовов стороны в списке симуляции.
#[derive(Debug, Clone, Copy)]
pub struct Tail {
    pub quote: usize,
    pub swap: usize,
    pub left: usize,
}

/// Вызов execute одним шагом пула на весь вход (минимум 1 единица: проба мерит, а не ограничивает).
/// Ошибки: NoRoute (ключ, шаг), NotConfigured (dex.vault, dex.deadline_s).
pub fn call_of(
    ctx: &Ctx,
    e: &Entry,
    k: ExecKind,
    (token_in, token_out, amount_in): (Address, Address, U256),
    executor: Address,
    now: u64,
) -> GatewayResult<ExecCall> {
    let (params, pool) = entry::singleton_step(ctx, e, k)?;
    let spec = StepSpec {
        params,
        pool,
        token_in,
        token_out,
        share_bps: ALL_BPS,
    };
    let step = kind::encode(&spec).map_err(|w| ctx.err(ErrorCode::NoRoute, w))?;
    Ok(ExecCall {
        executor,
        token_in,
        amount_in,
        token_out,
        min_out: U256::from(1u8),
        deadline: U256::from(now + ctx.int("dex.deadline_s")?),
        steps: vec![step],
        native: None,
        gated: Vec::new(),
    })
}

/// Разрешения кошелька исполнителю (все сразу: симуляция не знает текущих). Ошибки: как approve_executor::all.
pub fn approvals(
    ctx: &Ctx,
    list: &mut Vec<Call>,
    (wallet, x): (Address, Address),
    (token, amount): (Address, U256),
) -> GatewayResult<()> {
    for (to, input) in approve_executor::all(ctx, token, x, amount)? {
        list.push(Call {
            from: wallet,
            to,
            input,
        });
    }
    Ok(())
}

/// Квотер, execute, balanceOf(исполнитель) по токену входа и выхода. Ошибки: квотер (адрес, настройки).
pub fn tail(
    ctx: &Ctx,
    list: &mut Vec<Call>,
    e: &Entry,
    way: Way,
    (wallet, x): (Address, Address),
    c: &ExecCall,
) -> GatewayResult<Tail> {
    let (qto, qdata) = way.request(ctx, e, c.token_in, c.amount_in)?;
    let quote = list.len();
    list.push(Call {
        from: wallet,
        to: qto,
        input: qdata,
    });
    list.push(Call {
        from: wallet,
        to: x,
        input: c.data(),
    });
    list.push(pc::balance(wallet, c.token_in, x));
    list.push(pc::balance(wallet, c.token_out, x));
    Ok(Tail {
        quote,
        swap: quote + 1,
        left: quote + 2,
    })
}

/// Сторона для разбора. Принимает: окружение, пул, способ квотера, итоги, номера, (имя, токен входа, сумма). Ошибок нет.
pub fn side_of<'a>(
    ctx: &Ctx,
    e: &Entry,
    way: Way,
    res: &'a [CallResult],
    t: Tail,
    (name, token_in, amount): (&'static str, Address, U256),
) -> Side<'a> {
    let q = &res[t.quote];
    let quoted = if q.ok() {
        match way.decode(ctx, e, &q.return_data, token_in, amount) {
            Some(Ok(v)) => Some(v),
            _ => None,
        }
    } else {
        None
    };
    let bal = |i: usize| res.get(i).filter(|r| r.ok()).and_then(pc::u256);
    Side {
        name,
        swap: &res[t.swap],
        quoted,
        left: [bal(t.left), bal(t.left + 1)],
    }
}

/// Откат подготовки (перевод из пула-источника — баланс источника мог измениться за проход, разрешения) — помеха, не
/// итог пула: NoRoute (вызывающий оставляет прежний факт). None — подготовка прошла. Ошибок иных нет.
pub fn prep_failed(ctx: &Ctx, prep: &[CallResult]) -> Option<GatewayError> {
    let (i, r) = prep.iter().enumerate().find(|(_, r)| !r.ok())?;
    let why = format!(
        "подготовка пробы (вызов {i}: перевод встречного токена или разрешение исполнителю) откатилась: {}",
        r.why()
    );
    Some(ctx.err(ErrorCode::NoRoute, why))
}
