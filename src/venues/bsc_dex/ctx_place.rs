//! Файл: venues/bsc_dex/ctx_place.rs
//! Что делает: решение 435 (шаг 5б двух сетей, задача 433-г) — площадка окружения адаптера DEX: свой id (bsc_dex,
//! arb_dex…) и id всех площадок DEX процесса. Один код адаптера обслуживает несколько площадок DEX (по одной на сеть,
//! решение 433): id приходит из main.rs, а не из константы; по нему ищутся настройки площадки, журнал, сырые сообщения
//! и записи venue_state.
//! Сеть по умолчанию (решение 430): площадка DEX одна — её сеть, без чтения настроек; несколько — dex.default_chain
//! (default_chain). По ней каналы отправки без channel.chain относятся к сети по умолчанию (send/channels.rs, 433-е).
//! Кто вызывает: ctx.rs (venue, at, log), main.rs (создание окружений), send/channels.rs (serves), тесты (Place::BSC —
//! одна площадка bsc_dex).
//! От чего зависит: ctx.rs (чтение настроек).
//! Настройки: dex.chain [площадка DEX], dex.default_chain (вся система; только при нескольких площадках DEX); список
//! площадок DEX — dex.venues (читает main.rs).

use crate::gateway::errors::GatewayResult;
use crate::settings::read::At;

use super::ctx::Ctx;

/// Id первой площадки DEX (сеть BSC) — имя адаптера, под которым живут все прежние настройки, секреты и записи базы.
/// Это имя модуля, а не параметр: площадки других сетей получают свои id из настройки dex.venues (main.rs).
pub const BSC_ID: &str = "bsc_dex";

/// Площадка окружения: свой id и id всех площадок DEX процесса (строки живут весь процесс).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    /// Id этой площадки DEX (область «площадка» её настроек).
    pub id: &'static str,
    /// Id всех площадок DEX процесса, включая эту (по порядку регистрации; первая — bsc_dex).
    pub all: &'static [&'static str],
}

impl Place {
    /// Одна площадка bsc_dex — как до решения 435 (процесс без dex.venues, тесты на записях BSC).
    pub const BSC: Place = Place {
        id: BSC_ID,
        all: &[BSC_ID],
    };

    /// Площадка DEX в процессе одна. Ошибок нет.
    pub fn alone(&self) -> bool {
        self.all.len() <= 1
    }
}

impl Ctx {
    /// Сеть площадки — сеть по умолчанию (решение 430): площадка DEX в процессе одна — да, без чтения настроек (как до
    /// решения 435); несколько — dex.chain площадки равна dex.default_chain. Принимает: окружение.
    /// Ошибки: NotConfigured — при нескольких площадках не задана dex.chain или dex.default_chain.
    pub fn default_chain(&self) -> GatewayResult<bool> {
        if self.place.alone() {
            return Ok(true);
        }
        let own = self.text("dex.chain")?;
        let def = self
            .settings
            .text("dex.default_chain", At::default())
            .map_err(|e| self.tag(e))?;
        Ok(own.trim() == def.trim())
    }
}
