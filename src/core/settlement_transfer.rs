//! Файл: core/settlement_transfer.rs.
//! Что делает: связывает подтверждение вывода и кредит получателя.
//! Кто вызывает: исполнитель поручения.
//! От чего зависит: шлюзы выбранной сети и привязанный кошелёк.
//! Настройки: пределы вывода и маппинг кошельков.
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
    /// Сумма из подготовленной принятой заявки (в единицах счёта отправителя).
    #[serde(default)]
    pub debited: Option<Decimal>,
    pub sent: Transfer,
    pub credit: Transfer,
}

fn verified(
    (sender, recipient): (&super::types::Asset, &super::types::Asset),
    sent: Transfer,
    credit: Transfer,
    (from, to): (&VenueId, &VenueId),
    (from_net, network): (&str, &str),
    (budget, debited): (Decimal, Decimal),
) -> Result<Movement, String> {
    let tx = sent
        .tx_hash
        .as_deref()
        .filter(|v| !v.is_empty())
        .ok_or("У вывода конверсии нет хеша")?;
    if sent.venue != *from
        || sent.network != from_net
        || sent.venue_ref.is_empty()
        || !matches!(sent.status, TransferStatus::Sent | TransferStatus::Credited)
        || sent.asset.ticker != sender.ticker
        || sent
            .asset
            .chain
            .as_ref()
            .is_some_and(|c| Some(c) != sender.chain.as_ref())
        || sent
            .asset
            .contract
            .is_some_and(|a| Some(a) != sender.contract)
        || sent.amount <= Decimal::ZERO
        || sent.amount > debited
        || debited > budget
        || credit.status != TransferStatus::Credited
        || credit.venue != *to
        || credit.amount <= Decimal::ZERO
        || credit.asset.ticker != recipient.ticker
        || credit
            .asset
            .chain
            .as_ref()
            .is_some_and(|c| Some(c) != recipient.chain.as_ref())
        || credit
            .asset
            .contract
            .is_some_and(|a| Some(a) != recipient.contract)
        || credit.network != network
        || credit.amount > debited
        || !credit
            .tx_hash
            .as_deref()
            .is_some_and(|s| crate::gateway::tx_hash::same(s, tx))
    {
        return Err(
            "Нет зачисления расчётного актива по хешу вывода на указанном получателе".into(),
        );
    }
    Ok(Movement {
        sent,
        credit,
        debited: Some(debited),
    })
}

/// После перезапуска сначала проверяет исход исходного вывода. Новый баланс и новый адрес депозита
/// для поиска уже принятого вывода не нужны. При ещё не подтверждённом хеше оставляет шаг открытым.
pub async fn move_asset(
    core: &Core,
    id: &str,
    m: &super::settlement_market::Market,
    wallet: &DexAccountId,
    funding: bool,
    amount: Decimal,
) -> Result<Movement, String> {
    move_token(core, id, (m, &m.asset), wallet, funding, amount).await
}

/// Перевод только актива, закреплённого в поручении. Обратное направление проверяет собственный ввод/вывод.
pub(super) async fn move_token(
    core: &Core,
    id: &str,
    (m, token): (&super::settlement_market::Market, &super::types::Asset),
    wallet: &DexAccountId,
    from_cex: bool,
    amount: Decimal,
) -> Result<Movement, String> {
    current(core, id, (m, token), wallet, from_cex, amount, false).await
}

/// Сохранённое зачисление сверяется с площадками по прежней заявке. При отсутствии заявки
/// только ручная сверка, поскольку вывод уже мог состояться.
pub(super) async fn recheck_cash(
    core: &Core,
    p: &super::settlement_state::Plan,
    m: &super::settlement_market::Market,
    old: &Movement,
) -> Result<(), String> {
    let debited = old
        .debited
        .ok_or("У прежнего движения отсутствует исходное списание")?;
    let fresh = current(
        core,
        &p.deal,
        (m, &m.asset),
        &p.wallet,
        p.direction.buy_on == m.dex,
        debited,
        true,
    )
    .await?;
    if fresh.sent.venue_ref != old.sent.venue_ref
        || fresh.credit.venue_ref != old.credit.venue_ref
        || fresh.sent.tx_hash != old.sent.tx_hash
        || fresh.credit.tx_hash != old.credit.tx_hash
        || fresh.sent.amount != old.sent.amount
        || fresh.credit.amount != old.credit.amount
        || fresh.sent.fee != old.sent.fee
    {
        return Err("Изменился подтверждённый перевод или зачисление. Требуется сверка".into());
    }
    Ok(())
}

async fn current(
    core: &Core,
    id: &str,
    (m, token): (&super::settlement_market::Market, &super::types::Asset),
    wallet: &DexAccountId,
    from_cex: bool,
    amount: Decimal,
    read_only: bool,
) -> Result<Movement, String> {
    use super::transfers;
    m.ensure(core)?;
    let dex = m.dex.clone();
    let cex = &m.cex;
    let (from, to) = if from_cex { (cex, &dex) } else { (&dex, cex) };
    let from_asset = m.asset_on(core, token, from);
    let to_asset = m.asset_on(core, token, to);
    let deps = &core.inner.deps;
    let sender = deps
        .gw
        .get_as(from, Some(wallet))
        .await
        .map_err(|e| e.message)?;
    if !sender.supports_settlement_withdrawal() || id.is_empty() || amount <= Decimal::ZERO {
        return Err("Нет исполнимого вывода расчётного токена".into());
    }
    if !from_cex {
        let receiver = deps.gw.get_as(to, None).await.map_err(|e| e.message)?;
        if !receiver.supports_settlement_credit() {
            return Err("Для биржи не подтверждено прямое зачисление на торговый счёт".into());
        }
    }
    if let Some(req) = effect::previous_withdrawal(&deps.db, m, id, from, to, Some(wallet))
        .await
        .map_err(|e| e.message)?
    {
        if req.amount > amount || req.asset != *token || req.asset.ticker != from_asset.ticker {
            return Err("Ранее отправленный вывод не совпадает с бюджетом и активом этапа".into());
        }
        let created = effect::withdraw(
            &deps.db,
            (m, &from_asset),
            sender.as_ref(),
            &req,
            to,
            Some(wallet),
            None,
        )
        .await
        .map_err(|e| e.message)?;
        let sent = sender
            .withdrawal_status_for(&created)
            .await
            .map_err(|e| e.message)?;
        if sent.venue_ref != created.venue_ref
            || !matches!(sent.status, TransferStatus::Sent | TransferStatus::Credited)
        {
            return Err(format!(
                "Вывод {} находится в состоянии {:?}",
                created.venue_ref, sent.status
            ));
        }
        let tx = sent
            .tx_hash
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or("Биржа ещё не сообщила хеш вывода конверсии")?;
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
            .deposit_status_for(&to_asset, tx, &claim, None)
            .await
            .map_err(|e| e.message)?;
        let net = m.transfer_network(core, &to_asset, to)?;
        crate::gateway::settlement_proofs::pin(&deps.db, &m.dex, m.evm_chain_id, tx).await?;
        return verified(
            (&from_asset, &to_asset),
            sent,
            credit,
            (from, to),
            (&req.network, &net),
            (amount, req.amount),
        );
    }
    if read_only {
        return Err("Нет исходной заявки ранее подтверждённого перевода".into());
    }
    super::executor::buy_gate(core)?;
    if !super::registry_sync::enabled(core, cex) || !super::registry_sync::enabled(core, &dex) {
        return Err("Исходная биржа или конверсии выключены".into());
    }
    m.token_network(core, token, from_cex)?;
    let balance_key = if from_cex {
        from_asset.ticker.clone()
    } else {
        m.balance_key(core, token, from)
    };
    let free = super::limits::free_now(core, from, Some(wallet), &balance_key).await?;
    if amount > free {
        return Err(format!(
            "Для вывода требуется {amount} расчётного актива, свободно {free}"
        ));
    }
    let (sent, credit) = transfers::send_settlement(core, id, from, to, (m, token), amount, wallet)
        .await
        .map_err(|e| e.to_string())?;
    let net = m.transfer_network(core, &to_asset, to)?;
    let req = effect::previous_withdrawal(&deps.db, m, id, from, to, Some(wallet))
        .await
        .map_err(|e| e.message)?
        .ok_or("У вывода отсутствует исходная заявка")?;
    crate::gateway::settlement_proofs::pin(
        &deps.db,
        &m.dex,
        m.evm_chain_id,
        sent.tx_hash
            .as_deref()
            .ok_or("Нет хеша подтверждённого движения")?,
    )
    .await?;
    verified(
        (&from_asset, &to_asset),
        sent,
        credit,
        (from, to),
        (&req.network, &net),
        (amount, req.amount),
    )
}
