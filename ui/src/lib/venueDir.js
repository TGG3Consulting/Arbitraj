// Файл: ui/src/lib/venueDir.js
// Что делает: решение 242 — подраздел «Деньги и риск › по биржам и направлениям»: блоки (у каждой биржи — три рубрики
//   «биржа → DEX», «DEX → биржа», «биржа → биржа»; «DEX ↔ DEX» — один отдельный блок на все сделки DEX ↔ DEX, не по DEX
//   и не по кошельку), id области VenueDir («bingx:cex_dex», «dex_dex» — как src/settings/venue_dir.rs) и текст «что
//   действует», когда значение пары не задано: прежняя цепочка бэкенда (core/venue_dir.rs → Store::decimal_dir →
//   Store::decimal): площадка (у money.max_per_trade_usd — площадка покупки A, у profit.closure_share
//   (venue_role TransferTarget) — площадка продажи B), монета (profit.price_move_k), вся система. Дополнение 242 п. 2
//   (Тигран 09.11): у profit.price_move_k монета сильнее и заданной пары (settings/venue_dir.rs decimal_coin_dir).
// Кто вызывает: components/VenueDirMoney.svelte, pages/Settings.svelte, components/SettingsGroupList.svelte.
// От чего зависит: ничего (чистые функции над схемой, значениями и видами площадок из lib/settings.svelte.js).
// Настройки: money.min_profit_usd, money.min_profit_pct, money.max_per_trade_usd, profit.price_move_k,
//   profit.closure_share, risk.max_daily_loss_usd (решение 258), signal.probe_usd (решение 260) — все, у которых в схеме
//   есть область VenueDir.

/** Рубрики биржи (ключи — DirKind бэкенда). buy/sell — где покупка и продажа: 'self' — эта биржа, 'dex' — DEX,
 *  'cex' — другие биржи. */
export const CEX_KINDS = [
  { key: 'dex_cex', title: 'DEX → биржа', buy: 'dex', sell: 'self' },
  { key: 'cex_dex', title: 'биржа → DEX', buy: 'self', sell: 'dex' },
  { key: 'cex_cex', title: 'биржа → биржа (покупка на этой бирже)', buy: 'self', sell: 'cex' },
];
/** Блок «DEX ↔ DEX» — один на все циклы. */
export const DEX_DEX = { key: 'dex_dex', title: 'DEX ↔ DEX (все циклы)', buy: 'dex', sell: 'dex' };

/** Название подпункта (аргумент адреса — «<категория> › …»). */
export const SUB_TITLE = 'по биржам и направлениям';

/** Настройки с областью VenueDir в порядке схемы. Принимает: схему или null. Возвращает: [описание]. */
export const venueDirDefs = (schema) => (schema?.settings ?? []).filter((d) => d.scopes.includes('VenueDir'));

/** Категория-родитель подпункта — категория первой настройки с областью VenueDir ('' — таких нет). */
export const venueDirParent = (schema) => venueDirDefs(schema)[0]?.group ?? '';

/** Аргумент адреса подпункта («Деньги и риск › по биржам и направлениям»; '' — подпункта нет). */
export function venueDirRoute(schema) {
  const p = venueDirParent(schema);
  return p ? `${p} › ${SUB_TITLE}` : '';
}

/** id области: «площадка:тип», у dex_dex — «dex_dex» (src/settings/venue_dir.rs id). */
export const venueDirId = (venue, key) => (key === 'dex_dex' ? 'dex_dex' : `${venue}:${key}`);

/** Ключ области в SQLite — для истории одной строки (Scope::to_key). */
export const venueDirScopeKey = (venue, key) => `venue_dir:${venueDirId(venue, key)}`;

/**
 * Что действует, если значение пары не задано (прежняя цепочка бэкенда).
 * Принимает: описание, рубрику {buy, sell}, биржу блока ('' у DEX ↔ DEX), store.values, store.venueKinds.
 * Возвращает: текст.
 */
export function inheritedText(def, rub, venue, values, venueKinds) {
  const ids = (k) => venueKinds.filter((x) => x.kind === k).map((x) => x.id);
  const side = (who) => (who === 'self' ? [venue] : who === 'dex' ? ids('Dex') : ids('Cex').filter((v) => v !== venue));
  const global = values[`${def.name}|Global|`];
  const sys = def.scopes.includes('Global') ? `вся система: ${global ?? 'не задано'}` : 'не задано';
  if (def.scopes.includes('Venue')) {
    // Площадка прежней цепочки: получатель перевода (TransferTarget) — продажа B; решение 258 (PairVenue,
    // risk.max_daily_loss_usd) — биржа самой рубрики (у DEX ↔ DEX — DEX, core/day_pair.rs); иначе покупка A
    // (limits_parts.rs).
    const who = def.venue_role === 'TransferTarget' ? rub.sell
      : def.venue_role === 'PairVenue' ? (rub.key === 'dex_dex' ? 'dex' : 'self') : rub.buy;
    const list = side(who);
    if (!list.length) return sys;
    return list
      .map((v) => {
        const own = values[`${def.name}|Venue|${v}`];
        return own !== undefined ? `${v}: ${own} (площадка)` : `${v}: ${sys}`;
      })
      .join('; ');
  }
  if (def.scopes.includes('Coin')) return `значение монеты, если задано; иначе ${sys}`;
  return sys;
}
