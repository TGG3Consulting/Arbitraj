//! Файл: notify/press_event_exec.rs
//! Что делает: решение 408 (t193) — кнопка тревоги wallet.executor_v1 «Развернуть исполнитель» (xd.<площадка>.<кошелёк>;
//! вынесено из press_event_money.rs — размер файла): сначала оценка газа сейчас (контракт dex_executor_quote — как окно
//! ExecutorDeploy.svelte), есть предупреждение сборки — отказ (как недоступная «Подтвердить и отправить»), иначе вопрос
//! с владельцем, адресом, газом, ценой, стоимостью, байткодом и обёрткой → «Да» → команда ядра DeployExecutor
//! (core/executor_deploy.rs — тот же путь, что у интерфейса). Нажимает только человек; бот сам не разворачивает.
//! Кто вызывает: notify/press_event_money.rs (money_button).
//! От чего зависит: press_event.rs (pending, BAD), press.rs (Ctx), ctl/transfer.rs (n), шлюз (Registry::get_as).
//! Настройки: нет.

use crate::core::types::{DexAccountId, ManualAction, VenueId};

use super::ctl::transfer::n;
use super::mirror_state::Pending;
use super::press::Ctx;
use super::press_event::{BAD, pending};

impl Ctx<'_> {
    /// «Развернуть исполнитель»: оценка газа сейчас, затем вопрос. Ошибки: текст для нажавшего.
    pub(super) async fn ev_deploy(&self, arg: &str) -> Result<Pending, String> {
        let (venue, wallet) = arg.split_once('.').ok_or(BAD)?;
        let acc = DexAccountId(wallet.to_string());
        let v = self
            .core
            .inner
            .deps
            .gw
            .get_as(&VenueId(venue.to_string()), Some(&acc))
            .await
            .map_err(|e| e.message)?;
        let qt = v.dex_executor_quote().await.map_err(|e| e.message)?;
        if let Some(w) = &qt.warning {
            return Err(format!("Развернуть нельзя: {w}"));
        }
        let usdt = qt
            .cost_usdt
            .map(|u| format!(" ≈ {} USDT", n(u)))
            .unwrap_or_default();
        // Ревью t193 (Н1, М5): кошелёк по имени; исполнитель сейчас — должен быть v1 (кнопка тревоги wallet.executor_v1),
        // его адрес уходит в команду — ядро сверит его перед отправкой (уже переразвернули — отказ).
        let w = self
            .settings
            .accounts()
            .wallet(wallet)
            .ok_or_else(|| format!("Кошелька {wallet} нет"))?;
        if w.executor_version.as_deref() != Some("v1") {
            return Err(format!(
                "Исполнитель кошелька «{}» уже не v1 (версия {}) — переразвёртывание не нужно",
                w.name,
                w.executor_version.as_deref().unwrap_or("не сверена")
            ));
        }
        let old = w.executor.clone().unwrap_or_else(|| "нет".into());
        let q = format!(
            "Развернуть исполнитель кошелька «{}» ({wallet}) вместо v1 {old}?\nВладелец: {:#x}\nАдрес (при nonce {}): {:#x}\nГаз (лимит): {}, \
             цена газа {} wei\nСтоимость: {} BNB{usdt}\nБайткод: {} байт, SHA-256 {}, solc {}\nОбёртка BNB (навсегда): \
             {:#x}{}",
            w.name,
            qt.wallet,
            qt.nonce,
            qt.address,
            qt.gas,
            qt.gas_price_wei,
            n(qt.cost_bnb),
            qt.bytecode_len,
            qt.bytecode_sha256,
            qt.solc,
            qt.wnative,
            qt.note
                .as_ref()
                .map(|x| format!("\n{x}"))
                .unwrap_or_default()
        );
        let action = ManualAction::DeployExecutor {
            venue: venue.into(),
            wallet: wallet.into(),
            sha256: qt.bytecode_sha256.clone(),
            wnative: format!("{:#x}", qt.wnative),
            executor: w.executor.clone(),
        };
        Ok(pending(action, None, q))
    }
}
