//! Файл: core/drawdown_fact.rs
//! Что делает: решение 414 — площадка покупки направления и USDT на ней по факту баланса (для слепка).
//! Площадка покупки пары (п. 1 постановки t199): «биржа → DEX» и «биржа → биржа» — сама биржа; «DEX → биржа» —
//! DEX-кошелёк ЭТОЙ биржи (строка маппинга биржи, решение 132); «DEX ↔ DEX» — кошелёк строки «DEX ↔ DEX».
//! Факт — котируемая монета площадки (venue.quote_asset) так же, как остаток сделки в remain_fact.rs (решение 222):
//! биржа — свободно + заморожено ТОЛЬКО на торговом счёте (роль Trading, спот), только чтение; счёт «Фонд (Funding)» и
//! прочие счета не читаются и не считаются (решение Тиграна 21.11.2026: везде спот; счёт с ошибкой на Фонде не
//! блокирует слепок); счёт с ошибкой на торговом — факта нет; DEX — баланс кошелька (один balanceOf, решение
//! 113), свободно + заморожено (у кошелька заморожено 0 — сложение для одинакового счёта с биржей); строки USDT у
//! кошелька нет — «нет данных», ошибка, слепок не снимается (решение 140; ревью слияния 413×414, Н-2). Своё чтение, а не вызов remain_fact.rs: там — монета сделки и её счёт, здесь — USDT
//! направления (общая часть — account_balances / account_role шлюза).
//! Кто вызывает: drawdown_snap.rs.
//! От чего зависит: day_pair.rs, dex_account.rs (NO_WALLET), deal_words.rs (place), settings (accounts, venue.quote_asset),
//! шлюз (get_as, account_balances, account_role, balance).
//! Настройки: venue.quote_asset (площадки покупки), маппинг кошельков.

use rust_decimal::Decimal;

use crate::core::types::{Balance, DexAccountId, VenueId};
use crate::gateway::contract::{AccountRole, VenueKind};
use crate::settings::accounts::ROW_DEX_DEX;
use crate::settings::read::At;
use crate::settings::venue_dir::DirKind;

use super::engine::Core;

/// Площадка покупки направления: площадка, кошелёк DEX (у биржи — None), DEX ли.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub venue: VenueId,
    pub acc: Option<DexAccountId>,
    pub dex: bool,
}

impl Place {
    /// Тот же счёт: та же площадка; у DEX — тот же кошелёк (неизвестный кошелёк — считается тем же, как в
    /// usdt_hold.rs). Принимает: площадку и кошелёк сделки. Ошибок нет.
    pub fn same(&self, venue: &VenueId, acc: Option<&DexAccountId>) -> bool {
        &self.venue == venue
            && (!self.dex || self.acc.is_none() || acc.is_none() || self.acc.as_ref() == acc)
    }
}

/// Что делает: площадка покупки пары. Принимает: ядро, id пары. Возвращает: Place. Ошибки: не пара; DEX нет в
/// реестре; нет кошелька в маппинге (NO_WALLET).
pub fn place(core: &Core, p: &str) -> Result<Place, String> {
    let kind = super::day_pair::kind_of(p).ok_or_else(|| format!("{p}: не направление"))?;
    let wallet = |row: &str| -> Result<Place, String> {
        // Решение 430: площадка DEX сети по умолчанию (пара без сети — сеть по умолчанию; core/chain.rs).
        let dex = super::chain::default_dex(core)
            .ok_or_else(|| "DEX нет в реестре площадок".to_string())?;
        let acc = core
            .inner
            .deps
            .settings
            .accounts()
            .row(row)
            .ok_or_else(|| format!("{row}: {}", super::dex_account::NO_WALLET))?;
        Ok(Place {
            venue: dex,
            acc: Some(DexAccountId(acc)),
            dex: true,
        })
    };
    match (kind, super::day_pair::venue_of(p)) {
        (DirKind::CexToDex | DirKind::CexCex, Some(v)) => Ok(Place {
            venue: VenueId(v.to_string()),
            acc: None,
            dex: false,
        }),
        (DirKind::DexToCex, Some(v)) => wallet(v),
        (DirKind::DexDex, _) => wallet(ROW_DEX_DEX),
        _ => Err(format!("{p}: у направления нет биржи")),
    }
}

/// Площадка покупки словами (у DEX — с кошельком). Ошибок нет.
pub fn words(core: &Core, pl: &Place) -> String {
    super::deal_words::place(core, &pl.venue, pl.acc.as_ref())
}

/// Котируемая монета площадки (venue.quote_asset). Ошибки: не задана.
pub fn quote(core: &Core, v: &VenueId) -> Result<String, String> {
    let at = At {
        venue: Some(&v.0),
        coin: None,
    };
    core.inner
        .deps
        .settings
        .text("venue.quote_asset", at)
        .map_err(|e| e.message)
}

/// Что делает: USDT кошелька DEX по строке баланса (ревью слияния 413×414, Н-2). Решение 140 (low_usdt::free_of,
/// reconcile.rs): у кошелька DEX нет строки котируемой монеты — «нет данных», не 0 (список токенов адаптера DEX
/// приходит с подпиской рынка; до неё — только строка BNB): слепок не снимается, повтор — как при ошибке чтения.
/// Строка есть — свободно + заморожено, как у биржи (у кошелька заморожено по смыслу 0: balanceOf — весь баланс,
/// решение 113; сложение — чтобы обе ветки считали одинаково). Принимает: площадку (для текста), строку баланса,
/// котируемую монету. Возвращает: сумму. Ошибки: строки нет — «нет данных о USDT кошелька».
pub fn wallet_usdt(v: &VenueId, b: Option<&Balance>, q: &str) -> Result<Decimal, String> {
    b.map(|b| b.free + b.locked).ok_or_else(|| {
        format!(
            "{}: нет данных о {q} кошелька (строки {q} в балансах кошелька ещё нет — решение 140)",
            v.0
        )
    })
}

/// Что делает: USDT (котируемая монета) на площадке покупки по факту. Принимает: ядро, площадку.
/// Возвращает: сумму. Ошибки: не задан venue.quote_asset, площадка не подключена, сбой балансов, счёт не прочитан;
/// у кошелька DEX нет строки котируемой монеты — «нет данных» (wallet_usdt, решение 140).
pub async fn usdt(core: &Core, pl: &Place) -> Result<Decimal, String> {
    let q = quote(core, &pl.venue)?;
    let gw = core
        .inner
        .deps
        .gw
        .get_as(&pl.venue, pl.acc.as_ref())
        .await
        .map_err(|e| e.message)?;
    if gw.kind() != VenueKind::Cex {
        let b = gw
            .balance(&q)
            .await
            .map_err(|e| format!("{}: балансы: {}", pl.venue.0, e.message))?;
        return wallet_usdt(&pl.venue, b.as_ref(), &q);
    }
    let accs = gw
        .account_balances()
        .await
        .map_err(|e| format!("{}: балансы счетов: {}", pl.venue.0, e.message))?;
    let mut sum = Decimal::ZERO;
    // Решение Тиграна 21.11.2026: только торговый счёт (спот). Фонд (Funding) и прочие счета не читаем и не считаем —
    // ошибка чтения Фонда не должна блокировать слепок и торговлю (везде используется спот).
    for a in accs
        .iter()
        .filter(|x| gw.account_role(&x.account) == AccountRole::Trading)
    {
        if let Some(e) = &a.error {
            return Err(format!(
                "{}: счёт «{}» не прочитан: {e}",
                pl.venue.0, a.account
            ));
        }
        sum += a
            .balances
            .iter()
            .filter(|b| b.asset == q)
            .map(|b| b.free + b.locked)
            .sum::<Decimal>();
    }
    Ok(sum)
}
