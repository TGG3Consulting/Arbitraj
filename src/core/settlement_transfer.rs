//! Зачисление того же вывода native Base USDC на выбранном кошельке либо исходной бирже.
use super::settlement_effect as effect;
use crate::core::{
    engine::Core,
    types::{DexAccountId, Transfer, TransferStatus, VenueId},
};
use crate::gateway::contract::DepositClaim;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Movement {
    pub sent: Transfer,
    pub credit: Transfer,
}

fn verified(
    sent: Transfer,
    credit: Transfer,
    to: &VenueId,
    network: &str,
    budget: Decimal,
) -> Result<Movement, String> {
    let tx = sent
        .tx_hash
        .as_deref()
        .filter(|v| !v.is_empty())
        .ok_or("У вывода Base нет хеша")?;
    if credit.status != TransferStatus::Credited
        || credit.venue != *to
        || credit.amount <= Decimal::ZERO
        || credit.asset.ticker != "USDC"
        || credit.network != network
        || credit.amount > budget
        || !credit
            .tx_hash
            .as_deref()
            .is_some_and(|s| crate::gateway::tx_hash::same(s, tx))
    {
        return Err("Нет зачисления USDC по хешу вывода на указанном получателе".into());
    }
    Ok(Movement { sent, credit })
}

/// После перезапуска сначала проверяет исход исходного вывода. Новый баланс и новый адрес депозита
/// для поиска уже принятого вывода не нужны. При ещё не подтверждённом хеше оставляет шаг открытым.
pub async fn move_usdc(
    core: &Core,
    id: &str,
    cex: &VenueId,
    wallet: &DexAccountId,
    funding: bool,
    amount: Decimal,
) -> Result<Movement, String> {
    use super::{settlement, transfers};
    let dex = VenueId("base_dex".into());
    let (from, to) = if funding { (cex, &dex) } else { (&dex, cex) };
    let deps = &core.inner.deps;
    let sender = deps
        .gw
        .get_as(from, Some(wallet))
        .await
        .map_err(|e| e.message)?;
    if !sender.supports_settlement_withdrawal() || id.is_empty() || amount <= Decimal::ZERO {
        return Err("Нет исполнимого вывода Base USDC".into());
    }
    if let Some(req) = effect::previous_withdrawal(&deps.db, id, from, to, Some(wallet))
        .await
        .map_err(|e| e.message)?
    {
        if req.amount > amount || !effect::base_usdc(&req.asset) {
            return Err("Ранее отправленный вывод не совпадает с бюджетом и активом этапа".into());
        }
        let created = effect::withdraw(&deps.db, sender.as_ref(), &req, to, Some(wallet), None)
            .await
            .map_err(|e| e.message)?;
        let sent = sender
            .withdrawal_status_for(&created)
            .await
            .map_err(|e| e.message)?;
        if !matches!(sent.status, TransferStatus::Sent | TransferStatus::Credited) {
            return Err(format!(
                "Вывод {} находится в состоянии {:?}",
                created.venue_ref, sent.status
            ));
        }
        let tx = sent
            .tx_hash
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or("Биржа ещё не сообщила хеш вывода Base")?;
        let recipient = deps
            .gw
            .get_as(to, Some(wallet))
            .await
            .map_err(|e| e.message)?;
        let claim = DepositClaim {
            sender: from.clone(),
            withdrawal_id: created.venue_ref.clone(),
            deal: id.into(),
            expected: sender.withdrawal_wire(&created, &sent),
        };
        let credit = recipient
            .deposit_status_for(&req.asset, tx, &claim, None)
            .await
            .map_err(|e| e.message)?;
        let net = transfers::network(core, &req.asset, to)?;
        return verified(sent, credit, to, &net, amount);
    }
    super::executor::buy_gate(core)?;
    if !super::registry_sync::enabled(core, cex) || !super::registry_sync::enabled(core, &dex) {
        return Err("Исходная биржа или Base выключены".into());
    }
    let network = settlement::usdc_network(core, cex, funding)?;
    let free = super::limits::free_now(core, from, Some(wallet), "USDC").await?;
    if amount > free {
        return Err(format!(
            "Для вывода требуется {amount} USDC, свободно {free}"
        ));
    }
    let (sent, credit) = transfers::send_as(
        core,
        id,
        from,
        to,
        &network.asset,
        amount,
        Some(wallet),
        &mut vec![],
    )
    .await
    .map_err(|e| e.to_string())?;
    let net = transfers::network(core, &network.asset, to)?;
    verified(sent, credit, to, &net, amount)
}
