//! Файл: venues/bsc_dex/ctx_place.rs
//! Что делает: решение 435 (шаг 5б двух сетей, задача 433-г) — площадка окружения адаптера DEX: свой id (bsc_dex,
//! arb_dex…) и id всех площадок DEX процесса. Один код адаптера обслуживает несколько площадок DEX (по одной на сеть,
//! решение 433): id приходит из main.rs, а не из константы; по нему ищутся настройки площадки, журнал, сырые сообщения
//! и записи venue_state.
//! Сеть по умолчанию (решение 430): площадка DEX одна — её сеть, без чтения настроек; несколько — dex.default_chain
//! (default_chain). По ней каналы отправки без channel.chain относятся к сети по умолчанию (send/channels.rs, 433-е).
//! Ключ области «DEX» с сетью (433-з, план step5-plan.md 1.2): id DEX индексатора у площадки сети по умолчанию — как
//! есть (все живые настройки BSC прежние), у площадки другой сети — «id@сеть» (camelot@arbitrum): id GeckoTerminal у
//! сетей пересекаются. Перевод — на входе id от индексатора (directory/fetch.rs dex_key); вид пула Family.id — уже ключ,
//! все чтения области «DEX» идут по нему; свои ключи площадки — own_dex (families::all, dex_ids::extra).
//! Кто вызывает: ctx.rs (venue, at, log), main.rs (создание окружений), send/channels.rs (serves), directory/fetch.rs
//! (dex_key), pools/families.rs и pools/dex_ids.rs (dex_suffix, own_dex), тесты (Place::BSC — одна площадка bsc_dex).
//! От чего зависит: ctx.rs (чтение настроек), settings/dex_key.rs (правило суффикса сети — одно на площадку, интерфейс и
//! проверку ключа при записи, решение 490).
//! Настройки: dex.chain [площадка DEX], dex.default_chain (вся система; только при нескольких площадках DEX); список
//! площадок DEX — dex.venues (читает main.rs).

use crate::gateway::errors::GatewayResult;
use crate::settings::dex_key;
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
    /// Решение 490: это «суффикса ключа нет» — правило одно, settings/dex_key.rs suffix.
    pub fn default_chain(&self) -> GatewayResult<bool> {
        Ok(self.dex_suffix()?.is_none())
    }

    /// Суффикс ключа области «DEX» этой площадки (433-з): None — сеть по умолчанию (ключ = id индексатора, как раньше);
    /// Some(«@сеть») — другая сеть (dex.chain). Решение 490: правило — settings/dex_key.rs suffix (то же отдаёт
    /// интерфейсу GET /api/venues/{id}/dex-key-suffix). Принимает: окружение.
    /// Ошибки: NotConfigured — при нескольких площадках DEX не задана dex.chain или dex.default_chain.
    pub fn dex_suffix(&self) -> GatewayResult<Option<String>> {
        dex_key::suffix(
            self.place.alone(),
            || self.text("dex.chain"),
            || {
                self.settings
                    .text("dex.default_chain", At::default())
                    .map_err(|e| self.tag(e))
            },
        )
    }

    /// Решение 435 (433-к): окружение другой площадки DEX того же процесса — те же настройки, секреты, HTTP, журнал и
    /// сырые сообщения; своё: площадка, очередь узла, резервный узел, подтверждённые сети, справочник; без счёта.
    /// Принимает: id площадки (из dex.venues, строка живёт весь процесс). Ошибок нет.
    pub fn for_place(&self, id: &'static str) -> Ctx {
        Ctx {
            place: Place {
                id,
                all: self.place.all,
            },
            pace: Default::default(),
            failover: Default::default(),
            chains: Default::default(),
            index: Default::default(),
            evm_chain: Default::default(),
            account: None,
            ..self.clone()
        }
    }

    /// Ключ области «DEX» для id DEX индексатора (433-з): «id» у сети по умолчанию, «id@сеть» у другой.
    /// Принимает: id индексатора. Ошибки: как dex_suffix.
    pub fn dex_key(&self, raw: &str) -> GatewayResult<String> {
        let raw = key_of(raw, self.dex_suffix()?.as_deref());
        crate::settings::indexer_id::Bindings::read(&self.settings)
            .scope(&raw)
            .map(str::to_owned)
            .map_err(|why| self.err(crate::gateway::errors::ErrorCode::NotApplicable, why))
    }
}

/// Ключ области «DEX» по id индексатора и суффиксу площадки (None — сеть по умолчанию). Ошибок нет.
pub fn key_of(raw: &str, suffix: Option<&str>) -> String {
    match suffix {
        None => raw.to_string(),
        Some(s) => format!("{raw}{s}"),
    }
}

/// Ключ области «DEX» принадлежит площадке с этим суффиксом (433-з): сети по умолчанию — ключи без «@» (id индексатора
/// «@» не содержат), другой сети — ключи, оканчивающиеся на «@сеть». Принимает: ключ, суффикс. Ошибок нет.
pub fn own_dex(key: &str, suffix: Option<&str>) -> bool {
    match suffix {
        None => !key.contains('@'),
        Some(s) => key.len() > s.len() && key.ends_with(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ключи сети по умолчанию — id индексатора как есть; другой сети — с «@сеть»; чужие ключи не свои.
    #[test]
    fn dex_keys_split_networks() {
        assert_eq!(key_of("camelot", None), "camelot");
        assert_eq!(key_of("camelot", Some("@arbitrum")), "camelot@arbitrum");
        assert!(own_dex("pancakeswap_v2", None));
        assert!(!own_dex("camelot@arbitrum", None));
        assert!(own_dex("camelot@arbitrum", Some("@arbitrum")));
        assert!(!own_dex("pancakeswap_v2", Some("@arbitrum")));
        assert!(!own_dex("@arbitrum", Some("@arbitrum")));
    }
}
