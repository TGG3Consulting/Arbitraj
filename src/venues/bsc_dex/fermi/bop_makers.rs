//! Восстановление каждой доли bopAMM по MakerFill и двум фактическим ERC20-переводам того же маркетмейкера.
use super::slot::Filled;
use crate::venues::bsc_dex::send::receipt::Receipt;
use alloy::{
    primitives::{Address, U256},
    sol_types::SolEvent,
};

alloy::sol! {
    event MakerFill(uint8 indexed makerId,address indexed input,address indexed output,uint256 spent,uint256 received);
    event Transfer(address indexed from,address indexed to,uint256 value);
}
pub(super) fn paid(r: &Receipt, market: Address, m: &Filled, usdc: Address) -> bool {
    settled(r, market, m, usdc).is_some()
}
fn settled(r: &Receipt, market: Address, m: &Filled, usdc: Address) -> Option<()> {
    let direct = m.input == usdc || m.output == usdc;
    let mut flows = Vec::new();
    let mut stages = [(U256::ZERO, U256::ZERO); 2];
    for log in &r.logs {
        if log.topics().first() == Some(&Transfer::SIGNATURE_HASH)
            && [m.input, m.output, usdc].contains(&log.address())
        {
            let t = Transfer::decode_log_data(&log.inner.data).ok()?;
            flows.push((log.address(), t.from, t.to, t.value));
        }
        if log.address() != market || log.topics().first() != Some(&MakerFill::SIGNATURE_HASH) {
            continue;
        }
        // BopAMM._settle перечисляет обе суммы текущего maker и затем выпускает MakerFill.
        // Одна и та же сумма у следующего maker относится к следующему отрезку квитанции.
        let f = MakerFill::decode_log_data(&log.inner.data).ok()?;
        let stage = if (f.input, f.output) == (m.input, if direct { m.output } else { usdc }) {
            0
        } else if !direct && (f.input, f.output) == (usdc, m.output) {
            1
        } else {
            return None;
        };
        if f.spent.is_zero() || f.received.is_zero() {
            return None;
        }
        let to = if f.output == m.output {
            m.payer
        } else {
            market
        };
        let mut makers = flows
            .iter()
            .filter(|t| t.0 == f.input && t.1 == market && t.3 == f.spent)
            .map(|t| t.2)
            .filter(|maker| {
                flows
                    .iter()
                    .filter(|t| **t == (f.output, *maker, to, f.received))
                    .count()
                    == 1
            });
        let maker = makers.next()?;
        if maker.is_zero() || maker == market || makers.next().is_some() {
            return None;
        }
        flows.clear();
        stages[stage].0 = stages[stage].0.checked_add(f.spent)?;
        stages[stage].1 = stages[stage].1.checked_add(f.received)?;
    }
    (stages[0].0 == m.spent
        && !m.spent.is_zero()
        && !m.received.is_zero()
        && if direct {
            stages[0].1 == m.received
        } else {
            stages[0].1 == stages[1].0 && stages[1].1 == m.received
        })
    .then_some(())
}
