//! Файл: venues/bsc_dex/exec/executor/remeasure.rs
//! Что делает: решение 247 (Тигран 10.11: согласовано; живой случай PIG 09.11, сделка 20261109202626272PIG: «проба
//! исполнителя откатилась — код 3: execution reverted: minOut; доля 1: minOut 101.590829 USDT, ожидалось 102.101336
//! USDT, модельный налог монеты 4.0003%» — на каждом повторе и каждом нажатии «Продать»; руками продано за 101.1307)
//! — проба execute перед отправкой с пере-замером выхода. Факты сети PIG (квитанции ручных продаж через свой узел):
//! перевод PIG кошелёк → пул удержал ровно 5.0 %, модель (проба налога) — 4.0003 %, предпроверка симуляцией — выход
//! 102.16 при реальном ~101.1 — замер симуляцией может завысить выход, поэтому граница — наименьшее из замера и выхода
//! при налоге, наблюдённом в сети в этой сделке (bound_text). Проба откатилась ИМЕННО по minOut (require контракта
//! contracts/ArbExecutor.sol:120 `require(received >= minOut, "minOut")`; узел отдаёт код 3 и текст «execution
//! reverted: minOut» — факт журнала бота), продажа выхода (TradeRequest.exit_position) и ядро дало условия
//! (TradeRequest.remeasure) — та же продажа (тот же вызов, кошелёк, количество, маршрут, блок) симулируется с
//! наименьшим допустимым minOut — 1 единица токена (0 запрещён: exec/leg_min.rs, call.rs), возврат execute = реальный
//! выход R (прирост USDT у кошелька, меряет сам контракт). Граница min(R, выход при налоге сети) не ниже порога ядра —
//! minOut вызова = граница × (1 − exec.dex_slippage_pct)
//! (тот же допуск, что у модели: защита от сэндвича не слабее допуска, но от реального выхода на текущем блоке); ниже
//! — отказ с числами. Повторная проба не нужна: вызов тот же, блок тот же, отличие — только граница require, R ≥ M.
//! Сверка с req.worst_price после пере-замера не делается — граница теперь от R (худшая цена ядра — от той же модели,
//! что откатилась). Исполнитель проверяет только сумму (minOut = сумма долей, call.rs) — минимумы долей не меняются.
//! Строка «пере-замер: модель X, реально R, новый minOut M» — в журнал dex.remeasure и в пометку итога (хронология).
//! Решение 247, п. Б: откат по minOut у продажи выхода — монета во внеочередную перепроверку налогов (check/recheck.rs);
//! дополнение 10.11 (Тигран: согласовано) — перепроверка кошельком сделки (coins_by со счётом окружения сделки).
//! Решение 248 (кейс SMARS 10.11): замер с minOut 1 ед. делается при любом откате продажи выхода по minOut (и без
//! условий ядра — первая продажа сделки): граница по замеру уходит ядру (exec/exit_gap.rs → core/exit_gap.rs —
//! закрытие монеты для входа); minOut меняется только при условиях ядра (TradeRequest.remeasure), как в 247.
//! Симуляция — от кошелька сделки поверх живого блока (sim::run: один вызов execute, без пополнения кошелька от пула
//! exec/dry_fund.rs и без подмены состояния) — то есть с реального баланса кошелька после зачисления.
//! Кто вызывает: exec/executor/trade.rs.
//! От чего зависит: exec/executor/sim.rs, exec/exit_gap.rs, exec/txs.rs (Env), check/simulate.rs (CallResult),
//! check/recheck.rs, num.rs, ctx.rs.
//! Настройки: exec.dex_slippage_pct [bsc_dex].

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::super::super::check::simulate::CallResult;
use super::super::super::ctx::Ctx;
use super::super::super::num;
use super::super::exit_gap;
use super::super::txs::Env;
use super::call::ExecCall;
use super::sim;

/// Код отката узла (eth_simulateV1 CallResult.error.code: 3 — откат с данными, как в журнале бота «код 3: …»).
pub const REVERT_CODE: i64 = 3;
/// Текст узла при откате require(received >= minOut, "minOut") исполнителя (факт журнала бота 09.11, PIG).
pub const MIN_OUT_TEXT: &str = "execution reverted: minOut";

/// Откатилась ли проба по minOut исполнителя: код 3 и ровно текст «execution reverted: minOut» (другие require
/// контракта — «deadline», «owner», «out», «token call» … — свои тексты). Ошибок нет.
pub fn is_min_out(r: &CallResult) -> bool {
    !r.ok()
        && r.error
            .as_ref()
            .is_some_and(|e| e.code == REVERT_CODE && e.message == MIN_OUT_TEXT)
}

/// Новый minOut по реальному выходу: R × (1 − допуск/100) в единицах USDT (вниз, ≤ R). Принимает: реальный выход
/// (десятичный), допуск %, decimals USDT. Возвращает: None — не переводится или 0. Ошибок нет.
pub fn min_out_of(real: Decimal, slip_pct: Decimal, dec: u8) -> Option<U256> {
    let m = real * (Decimal::ONE - slip_pct / Decimal::ONE_HUNDRED);
    num::to_units(m, dec).filter(|u| !u.is_zero())
}

/// Выход модели при налоге, наблюдённом в сети (решение 247; факты PIG 09.11: проба налога 4.0003 %, в сети ровно
/// 5.0 %): по долям Σ E_k × (100 − max(t_сеть, t_k)) / (100 − t_k), где E_k — ожидаемый выход доли при модельном
/// налоге t_k её шага. Это НИЖНЯЯ граница выхода при налоге t_сеть (при тех же пулах): вход в пул меньше в
/// (100 − t_сеть) / (100 − t_k) раз, а выход пула (и цепочки пулов) вогнут по входу и равен 0 при нуле — уменьшенный
/// вход даёт не меньше пропорционального выхода. Потому граница min(замер, эта оценка) — налог берётся наибольший из
/// модельного, сетевого и симуляции. Пример PIG: 102.101336 × 95 / 95.9997 = 101.0380… ≤ 101.1307 (ручная продажа).
/// Принимает: по долям (ожидаемый выход, модельный налог %), налог в сети %. Возвращает: None — налог в сети не
/// наблюдался или модельный налог ≥ 100 %. Ошибок нет.
pub fn seen_out(exp: &[(Decimal, Decimal)], seen: Option<Decimal>) -> Option<Decimal> {
    let t = seen?;
    let hundred = Decimal::ONE_HUNDRED;
    exp.iter().try_fold(Decimal::ZERO, |acc, (e, tk)| {
        (*tk < hundred).then(|| acc + *e * (hundred - t.max(*tk)) / (hundred - *tk))
    })
}

/// Проба execute с пере-замером. Принимает: запрос, вызов (minOut меняется при пере-замере), кошелёк, (номер, время)
/// блока сборки, (decimals монеты, USDT), текст долей для отката (sim::legs_text), по долям (ожидаемый выход,
/// модельный налог %), окружение отправки (очереди перепроверок — п. Б; канал событий ядру — решение 248).
/// Граница выхода — наименьшее из: замера симуляцией R (minOut 1 ед.) и выхода модели при налоге, наблюдённом в сети в
/// этой сделке (seen_out; TradeRequest.remeasure.tax_seen_pct) — налог берётся наибольший из модельного, сетевого и
/// симуляции. minOut = граница × (1 − exec.dex_slippage_pct), не 0.
/// Возвращает: None — проба прошла как раньше; Some(строка) — пере-замер: minOut вызова заменён, строка — для
/// хронологии. Ошибки: как sim::check (откат — NoRoute с текстом долей); пере-замер не помог — та же ошибка отката с
/// числами пере-замера (код NoRoute: до отправки, ядро решает, повторять ли — решения 110, 235).
#[allow(clippy::too_many_arguments)]
pub async fn probe(
    ctx: &Ctx,
    req: &TradeRequest,
    c: &mut ExecCall,
    wallet: Address,
    at: (u64, u64),
    decs: (u8, u8),
    diag: &str,
    exp: &[(Decimal, Decimal)],
    env: &Env,
) -> GatewayResult<Option<String>> {
    let r = sim::run(ctx, c, wallet, at).await?;
    let exit = req.side == Side::Sell && req.exit_position;
    if !(exit && is_min_out(&r)) {
        return sim::judge(ctx, req, c, &r, decs)
            .map(|_| None)
            .map_err(|e| sim::explain(e, diag));
    }
    let orig = sim::explain(no_route(ctx, &r), diag);
    // Решение 247, п. Б: модель налога монеты не сошлась с сетью — внеочередная перепроверка (для следующих сделок).
    if let Some(a) = req.asset.bsc_contract {
        // Дополнение 10.11 (Тигран: согласовано): перепроверка — кошельком сделки (счёт окружения сделки), не строки «Пробы».
        env.wake.coins_by(&[a], env.ctx.account.as_deref());
        let msg = format!(
            "{} {}: откат продажи выхода по minOut — монета в перепроверку налогов (решение 247, п. Б); кошельком сделки «{}»",
            req.deal_id,
            req.asset.ticker,
            env.ctx.wallet_name()
        );
        ctx.log(Level::Info, "dex.tax_recheck", msg, None).await;
    }
    let usdt_dec = decs.1;
    let old = num::to_decimal(c.min_out, usdt_dec).unwrap_or_default();
    let mut probe = c.clone();
    probe.min_out = U256::from(1u8);
    let r1 = match sim::run(ctx, &probe, wallet, at).await {
        Ok(r1) => r1,
        Err(e) => {
            return Err(add(
                orig,
                &format!("пере-замер не выполнен — {}", e.message),
            ));
        }
    };
    if !r1.ok() {
        let why = format!(
            "пере-замер: симуляция с minOut 1 ед. откатилась — {}",
            r1.why()
        );
        return Err(add(orig, &why));
    }
    let got = sim::received(ctx, &r1).map_err(|e| add(orig.clone(), &e.message))?;
    let real = num::to_decimal(got, usdt_dec)
        .ok_or_else(|| add(orig.clone(), "пере-замер: выход не число"))?;
    let seen = req.remeasure.as_ref().and_then(|r| r.tax_seen_pct);
    let (bound, head) = bound_text(exp, real, seen);
    // Решение 248: граница по замеру — ядру (закрыть монету для входа, если ниже модели больше guard.exit_gap_pct).
    exit_gap::report(ctx, &env.stats, req, exp, bound);
    let Some(rm) = req.remeasure.as_ref() else {
        let why = format!(
            "{head} — повтор с новым minOut не разрешён ядром (первая продажа сделки или продажа без порога)"
        );
        return Err(add(orig, &why));
    };
    let slip = ctx.dec("exec.dex_slippage_pct")?;
    if bound < rm.floor {
        let why = format!(
            "{head} — граница ниже {} USDT (порог убытка, по которому решено продать), повтор не сделан",
            rm.floor.round_dp(6).normalize()
        );
        return Err(add(orig, &why));
    }
    let Some(m) = min_out_of(bound, slip, usdt_dec) else {
        let why = format!("{head} — новый minOut 0, обмен без минимума не отправляется");
        return Err(add(orig, &why));
    };
    c.min_out = m;
    let new = num::to_decimal(m, usdt_dec).unwrap_or_default();
    let line = format!(
        "{head}, новый minOut {} USDT (был {}; допуск {slip}%)",
        new.normalize(),
        old.normalize()
    );
    let msg = format!(
        "{} {} {}: {line} (решение 247)",
        req.deal_id, req.qty, req.asset.ticker
    );
    ctx.log(Level::Warn, "dex.remeasure", msg, None).await;
    Ok(Some(line))
}

/// Граница выхода пере-замера и её текст: наименьшее из замера `real` и выхода при налоге в сети (seen_out). Текст —
/// «пере-замер: модель X USDT (налог t%), реально R USDT[, в сети налог Z% → W USDT][; симуляция занижает налог: …]».
/// Принимает: по долям (ожидаемый, налог %), замер, налог в сети %. Возвращает: (граница, текст). Ошибок нет.
pub fn bound_text(
    exp: &[(Decimal, Decimal)],
    real: Decimal,
    seen: Option<Decimal>,
) -> (Decimal, String) {
    let model: Decimal = exp.iter().map(|(e, _)| *e).sum();
    let t_model = exp
        .iter()
        .map(|(_, t)| *t)
        .fold(Decimal::ZERO, Decimal::max);
    let r6 = |d: Decimal| d.round_dp(6).normalize();
    let mut head = format!(
        "пере-замер: модель {} USDT (налог {}%), реально {} USDT",
        r6(model),
        t_model.round_dp(4).normalize(),
        r6(real)
    );
    let mut bound = real;
    if let (Some(w), Some(t)) = (seen_out(exp, seen), seen) {
        head.push_str(&format!(
            ", в сети налог {}% → не больше {} USDT",
            t.round_dp(4).normalize(),
            r6(w)
        ));
        if w < real {
            head.push_str(&format!(
                "; симуляция занижает налог: модель {} USDT, симуляция {} USDT, в сети налог {}% — граница по сети",
                r6(model),
                r6(real),
                t.round_dp(4).normalize()
            ));
            bound = w;
        }
    }
    (bound, head)
}

/// Ошибка отката пробы (как sim::received: NoRoute «проба исполнителя откатилась — …»). Ошибок нет.
fn no_route(ctx: &Ctx, r: &CallResult) -> GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        format!("проба исполнителя откатилась — {}", r.why()),
    )
}

/// Дописывает к ошибке строку пере-замера. Ошибок нет.
fn add(mut e: GatewayError, what: &str) -> GatewayError {
    e.message = format!("{}; {what}", e.message);
    e
}
