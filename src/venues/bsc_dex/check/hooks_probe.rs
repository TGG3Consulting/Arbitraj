//! Файл: venues/bsc_dex/check/hooks_probe.rs
//! Что делает: влияние крючка пула V4 группы B/C (решение 32) — пробный обмен квотером своей DEX (он исполняет
//! крючки) против локальной математики пула без крючка: покупка (встречный токен → монета) и продажа (монета →
//! встречный токен). Влияние, % = (математика − квотер) / математика · 100; положительное — получаем меньше. Откат
//! квотера NotEnoughLiquidity(PoolId) — точка пропускается (на такой сумме пул не торгуется; все точки стороны — причина
//! «в пуле не хватает ликвидности для пробы»), иной откат — ошибка (проверка крючка не прошла).
//! Решение «335 (В1)»: только на одном блоке — состояние пула на блоке и квотер eth_call на тот же блок.
//! Решение «335 (лесенка влияния)»: влияние — функция суммы и стороны. План пробы (Plan) — точки стороны: вход в пул
//! (минимальные единицы токена входа) и сумма в USDT; итог — HookImpact с лесенкой buy / sell (точки по возрастанию
//! входа) и наибольшим влиянием стороны buy_pct / sell_pct. Все вызовы квотера плана — одним пакетом (node/call.rs
//! try_many: Multicall3 или пачка JSON-RPC, один круг до узла; ответ пакета = отдельным вызовам — запись узла 17.11,
//! tests/bsc_dex_hook_pools_batch_1117.rs); пул с виртуальным квотером (решение 186) — по одному (quote/quoter.rs out_at).
//! - ladder: план цикла крючков по суммам в USDT (check/hook_points.rs): покупка — сумма во встречном токене
//!   (решение 52, quote/rate.rs), продажа — монеты на сумму по цене пула на блоке (quote/spot.rs, как сверка);
//! - measure_plan: план → вызовы квотера → влияние по точкам (check/hooks_judge.rs judge_plan; вынесено ревью 4 —
//!   файл был больше 200 строк; здесь реэкспорт judge_plan, settled, impact) по состоянию пула на блоке.
//!
//! Кто вызывает: check/hooks_head.rs (цикл крючков), check/quoter_reprobe.rs (перемер перед паузой — одна точка),
//! check/hook_batch.rs (проба перед сделкой — точка на сумме сделки; judge_plan, requests).
//! От чего зависит: quote/quoter.rs, quote/quoter_calls.rs, quote/pool_quote.rs, quote/rate.rs, quote/spot.rs,
//! check/hook_batch_read.rs (quoted), quote/virtual_quoter.rs, node/call.rs, tokens.rs, num.rs.
//! Настройки: dex.quoter [вид пула]; dex.multicall3 [bsc_dex] (через try_many).
//!
//! Квотер выполняет настоящий swap внутри vault.lock и откатывается — IP/src/pool-cl/lens/CLQuoter.sol#L27-L38
//! (research/flow/bsc_v4.md, раздел 5): «hook-и пула в квоте исполняются».

use alloy_primitives::{Address, Bytes, U256};
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Asset, CheckState, HookGroup, HookImpact, PoolCheck};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::num;
use super::super::pools::registry::{Entry, Registry};
use super::super::quote::{quoter, quoter_calls, quoter_revert, rate, spot, virtual_quoter};
use super::super::tokens::Tokens;
use super::hook_batch_read::quoted;

pub use super::hooks_judge::{impact, judge_plan, settled};

/// Итог пробы: влияние или причина, почему не посчитать.
pub type Measured = GatewayResult<Result<HookImpact, String>>;

/// Ответ квотера по точке: выход или причина «не хватает ликвидности»; ошибка — откат или узел.
pub type Quoted = GatewayResult<Result<U256, String>>;

/// Точка плана: вход в пул (минимальные единицы токена входа стороны) и сумма в USDT (0 — не считалась).
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub usd: Decimal,
    pub amount_in: U256,
}

/// План пробы: точки покупки (вход — встречный токен пула) и продажи (вход — монета), по возрастанию входа.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    pub buy: Vec<Point>,
    pub sell: Vec<Point>,
}

/// Итог проверки крючка пула для ядра и экрана (вынесено решением 335 — тот же текст полей для пробы на блоке H).
/// Принимает: окружение, запись, группу крючка, итог влияния. Возвращает: PoolCheck со временем «сейчас». Ошибок нет.
pub fn pool_check(
    ctx: &Ctx,
    e: &Entry,
    group: HookGroup,
    impact: CheckState<HookImpact>,
) -> PoolCheck {
    PoolCheck {
        venue: ctx.venue(),
        family: e.family.id.to_string(),
        pool: e.pool.reference.clone(),
        asset: Asset {
            chain: None,
            ticker: e.ticker.clone(),
            contract: Some(e.coin),
        },
        group,
        impact,
        checked_at: Some(Utc::now()),
    }
}

/// Начало причины «в пуле не хватает ликвидности для пробы» — не провал крючка, а свойство пула на сумму пробы:
/// так причина и показывается (check/quoter_verify.rs, quote/select.rs, check/cycle.rs — без «проверка крючка не
/// прошла»).
pub const SHORT: &str = "в пуле не хватает ликвидности для пробы check.probe_usdt";

/// Причина по откату квотера NotEnoughLiquidity(PoolId). Ошибок нет.
pub fn short(id: alloy_primitives::B256) -> String {
    format!("{SHORT} (NotEnoughLiquidity, PoolId {id})")
}

/// Что делает: план цикла крючков (см. шапку). Принимает: окружение, токены, реестр, запись на блоке, суммы покупки и
/// продажи в USDT (check/hook_points.rs). Возвращает: план; Ok(Err) — сумму не перевести (причина). Ошибки: узел
/// (decimals), не задан тикер USDT.
pub async fn ladder(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    e: &Entry,
    (buy_usd, sell_usd): (&[Decimal], &[Decimal]),
) -> GatewayResult<Result<Plan, String>> {
    let usdt = tokens.usdt(ctx)?;
    let dec = tokens.decimals(ctx, usdt).await?;
    let counter = |usd: Decimal| -> Result<U256, String> {
        let units = num::to_units(usd, dec)
            .ok_or_else(|| format!("{usd} USDT не переводится в единицы"))?;
        rate::from_usdt(ctx, reg, usdt, e.usdt, units).map_err(|err| err.message)
    };
    let mut plan = Plan::default();
    for usd in buy_usd {
        match counter(*usd) {
            Ok(amount_in) => plan.buy.push(Point {
                usd: *usd,
                amount_in,
            }),
            Err(why) => return Ok(Err(why)),
        }
    }
    for usd in sell_usd {
        let coins = match counter(*usd) {
            Ok(c) => spot::coins_for(e, c),
            Err(why) => return Ok(Err(why)),
        };
        match coins.filter(|c| !c.is_zero()) {
            Some(amount_in) => plan.sell.push(Point {
                usd: *usd,
                amount_in,
            }),
            None => {
                return Ok(Err(
                    "цены пула нет — монет на сумму пробы продажи не посчитать".into(),
                ));
            }
        }
    }
    Ok(Ok(plan))
}

/// Вызовы квотера плана: сначала покупка, затем продажа. Ошибки: NotConfigured (адрес квотера), вид пула.
pub fn requests(ctx: &Ctx, e: &Entry, plan: &Plan) -> GatewayResult<Vec<(Address, Bytes)>> {
    let buy = plan
        .buy
        .iter()
        .map(|p| quoter_calls::request(ctx, e, e.usdt, p.amount_in));
    let sell = plan
        .sell
        .iter()
        .map(|p| quoter_calls::request(ctx, e, e.coin, p.amount_in));
    buy.chain(sell).collect()
}

pub(super) fn individual(ctx: &Ctx, e: &Entry) -> bool {
    matches!(e.family.variant, super::super::pools::family_variant::Variant::SupernovaIntegral | super::super::pools::family_variant::Variant::CypherIntegral | super::super::pools::family_variant::Variant::PuzzleRanges)
        || virtual_quoter::on(ctx, e.family.id, e.family.proto)
}

/// Что делает: проба плана на блоке `tag` по состоянию `e` на этом блоке (см. шапку). Возвращает: Measured.
/// Ошибки: узел, настройки, откат квотера (кроме «не хватает ликвидности»).
pub async fn measure_plan(ctx: &Ctx, e: &Entry, plan: &Plan, tag: &str) -> Measured {
    let n = plan.buy.len() + plan.sell.len();
    let mut remote = Vec::with_capacity(n);
    if e.family.variant == super::super::pools::family_variant::Variant::PuzzleRanges {
        let calls=requests(ctx,e,plan)?;
        let answers=call::independent(ctx,&calls,tag).await?;
        remote.extend((0..n).map(|i| quoted(ctx,e,answers.get(i))));
    } else if individual(ctx, e) {
        let ins = plan.buy.iter().map(|p| (e.usdt, p.amount_in));
        for (token_in, amount) in ins.chain(plan.sell.iter().map(|p| (e.coin, p.amount_in))) {
            remote.push(match quoter::out_at(ctx, e, token_in, amount, tag).await {
                Ok(v) => Ok(Ok(v)),
                Err(err) => match quoter_revert::short_pool(&err) {
                    Some(id) => Ok(Err(short(id))),
                    None => Err(err),
                },
            });
        }
    } else {
        let calls = requests(ctx, e, plan)?;
        let ans = call::try_many(ctx, &calls, tag).await?;
        remote.extend((0..n).map(|i| quoted(ctx, e, ans.get(i))));
    }
    judge_plan(ctx, e, plan, remote)
}
