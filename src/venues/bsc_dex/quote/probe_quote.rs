//! Файл: venues/bsc_dex/quote/probe_quote.rs
//! Что делает: решение 388 (задача t174) — «ожидалось без налога» в симуляции пробы налогов (check/probe.rs): у DEX с
//! включённым виртуальным квотером (virtual_quoter::on — dex.quoter_virtual = да и вид поддержан) — вызов виртуального
//! квотера (quoteExactIn / quoteMetric / quoteIzi / quoteSmardex, quote/virtual_quoter.rs request) по его адресу с подменой
//! кода в той же eth_simulateV1 (как хвост сухой сделки, exec/dry_surplus_tail.rs, решение 186); иначе — как раньше:
//! квотер DEX или, у V2-форка без роутера, резервы пары (quote/v2_pair_probe.rs request_any / decode_any).
//! До решения 388 проба шла только в quote/quoter_calls.rs, и у видов «только виртуальный квотер» (Metric OMM, Algebra
//! Integral, iZiSwap / Biswap V3, SmarDex) налоги пробой не мерились никогда (ошибка «не задана настройка» при заданной).
//! Способ выбирается ОДИН раз на пробу (Way::of) — вызов и разбор ответа одного и того же квотера, даже если настройку
//! поменяют между симуляциями.
//! Ревью 388 (Н1): виртуальный квотер пробной покупки откатился, а сама покупка прошла — помеха измерения (NoRoute), не
//! вердикт по монете (quote_reverted).
//! Кто вызывает: check/probe.rs (run), check/probe_calls.rs (sell_result).
//! От чего зависит: quote/{virtual_quoter, v2_pair_probe}.rs, check/simulate.rs (Override), pools/registry.rs.
//! Настройки: dex.quoter_virtual [DEX]; через v2_pair_probe — dex.router / dex.quoter [вид пула], dex.v2_fee_pct [DEX].
//!
//! Виртуальный квотер выдаёт выход БЕЗ налога токена, как QuoterV2 (решение 146): swap самого пула, откат в обратном
//! вызове ДО перевода входного токена в пул (contracts/VirtualQuoter.sol, try/catch — сам вызов не откатывается,
//! состояние симуляции не меняется). Адрес подмены — virtual_quoter::address() (вычисляется, не настройка).

use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::check::simulate::{CallResult, Override};
use super::super::ctx::Ctx;
use super::super::pools::registry::Entry;
use super::{v2_pair_probe, virtual_quoter};

/// Каким квотером проба берёт «ожидалось без налога»: virt = true — виртуальный квотер с подменой кода.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Way {
    /// Виртуальный квотер (решение 186) включён у DEX пула и умеет его вид.
    pub virt: bool,
}

impl Way {
    /// Способ для пула: virtual_quoter::on (dex.quoter_virtual [DEX] = да и вид поддержан). Ошибок нет (не задано — нет).
    pub fn of(ctx: &Ctx, e: &Entry) -> Way {
        Way {
            virt: virtual_quoter::on(ctx, e.family.id, e.family.proto),
        }
    }

    /// Вызов «ожидалось без налога» для симуляции. Принимает: окружение, пул, токен входа, сумму. Возвращает: (адрес,
    /// данные). Ошибки: Internal (виртуальный: пул не контракт, у Metric не прочитан порядок токенов); иначе — как у
    /// v2_pair_probe::request_any (NotConfigured — адрес квотера / комиссия, Internal).
    pub fn request(
        self,
        ctx: &Ctx,
        e: &Entry,
        token_in: Address,
        amount: U256,
    ) -> GatewayResult<(Address, Bytes)> {
        if !self.virt {
            return v2_pair_probe::request_any(ctx, e, token_in, amount);
        }
        let (_, input) = virtual_quoter::request(e, token_in, amount)
            .map_err(|m| ctx.err(ErrorCode::Internal, format!("{}: {m}", e.family.title)))?;
        Ok((virtual_quoter::address(), input))
    }

    /// Подмены кода для симуляции: виртуальный — код квотера по его адресу; иначе пусто. Ошибки: Internal (код квотера
    /// не прошёл сверку хеша contracts/out/VirtualQuoter.sha256 или не hex).
    pub fn overrides(self, ctx: &Ctx) -> GatewayResult<Vec<Override>> {
        if !self.virt {
            return Ok(Vec::new());
        }
        let code = virtual_quoter::runtime().map_err(|m| ctx.err(ErrorCode::Internal, m))?;
        Ok(vec![Override {
            address: virtual_quoter::address(),
            code,
            state: Vec::new(),
        }])
    }

    /// Разбор ответа request. Принимает: окружение, пул, сырой ответ, токен входа, сумму. Возвращает: Some(Ok(выход)),
    /// Some(Err(virtual_quoter::SHORT)) — виртуальный квотер: пул взял не весь вход (ликвидности не хватает); None — не
    /// по ABI (или у V2-форка без роутера комиссия не задана). Ошибок нет.
    pub fn decode(
        self,
        ctx: &Ctx,
        e: &Entry,
        raw: &[u8],
        token_in: Address,
        amount: U256,
    ) -> Option<Result<U256, &'static str>> {
        if self.virt {
            virtual_quoter::decode(raw, amount)
        } else {
            v2_pair_probe::decode_any(ctx, e, raw, token_in, amount).map(Ok)
        }
    }

    /// «Ожидалось без налога» пробной покупки. Возвращает: Some(выход); None — не по ABI. Ошибки: NoRoute — виртуальный
    /// квотер: ликвидности не хватает (то же, что откат QuoterV2 «не хватает ликвидности» на шаге квотера —
    /// check/probe_calls.rs buy_revert: свойство пула на эту сумму, монета не голосует).
    pub fn expected(
        self,
        ctx: &Ctx,
        e: &Entry,
        raw: &[u8],
        token_in: Address,
        amount: U256,
    ) -> GatewayResult<Option<U256>> {
        match self.decode(ctx, e, raw, token_in, amount) {
            Some(Ok(v)) => Ok(Some(v)),
            Some(Err(why)) => Err(ctx.err(
                ErrorCode::NoRoute,
                format!("{}: квотер пробной покупки — {why}", e.family.title),
            )),
            None => Ok(None),
        }
    }

    /// Ревью 388 (Н1): откат вызова виртуального квотера пробной покупки при прошедшей покупке. Квотер откатывается по
    /// причине самого пула на этом блоке (цена оракула Metric устарела, пул на паузе, «VQ: no callback» — обратного
    /// вызова нет в коде квотера), а обмен тем же пулом в той же симуляции прошёл — монета ни при чём: налоги пула не
    /// измерены, это помеха (NoRoute — check/taxes.rs «не проверено», повтор), а не вердикт «не прошла»
    /// (check/probe_calls.rs buy_revert). Квотер DEX (не виртуальный) — как раньше (None). Принимает: окружение, пул,
    /// (номер первого отката, номер вызова квотера), итог отката, итог покупки (None — его нет в ответе: решать как раньше). Возвращает: ошибку или None — решать
    /// как раньше. Ошибок иных нет.
    pub fn quote_reverted(
        self,
        ctx: &Ctx,
        e: &Entry,
        (i, quote): (usize, usize),
        r: &CallResult,
        buy: Option<&CallResult>,
    ) -> Option<GatewayError> {
        if !self.virt || i != quote || !buy.is_some_and(|b| b.ok()) {
            return None;
        }
        Some(ctx.err(
            ErrorCode::NoRoute,
            format!(
                "{}: виртуальный квотер пробной покупки откатился (шаг {i}), а сама покупка прошла — помеха \
                 измерения, налоги пула не измерены (решение 388): {}",
                e.family.title,
                r.why()
            ),
        ))
    }
}
