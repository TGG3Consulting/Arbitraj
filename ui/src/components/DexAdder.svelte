<!--
  Файл: ui/src/components/DexAdder.svelte
  Что делает: решение 490 — «Добавить DEX» в карточке настройки области «DEX» (экран «Настройки»): DEX, которой нет в
    списке адаптера (её пулы находит только поиск по фабрике — например DEX Arbitrum без индексатора), вводится руками.
    Человек выбирает площадку DEX (площадки вида Dex из GET /api/venues/kinds; одна — выбора нет, как раньше) и вводит
    только сырой id (правило — checkDexId, без «@»). Ключ области «DEX» = id + суффикс сети ЭТОЙ площадки, суффикс даёт
    сервер (GET /api/venues/{id}/dex-key-suffix — то же правило, что у площадки, settings/dex_key.rs): «@сеть» руками не
    печатается, опечатки в имени сети нет. Сеть по умолчанию — суффикса нет (ключ = id, как у BSC раньше).
    Причина отказа (суффикс не определить, длина ключа) — красным рядом. Новый ключ «id@сеть» сервер ещё раз проверяет
    при первом сохранении значения (api/settings.rs set) — причина показывается в строке значения (SettingEditor).
  Кто вызывает: components/SettingCard.svelte (onadd — добавить строку с этим ключом).
  От чего зависит: lib/api.js, lib/settings.svelte.js (store.venueKinds, через checkDexId — dex.id_max_len),
    lib/dex.svelte.js (checkDexId), lib/uniq.js.
  Настройки: dex.id_max_len (через checkDexId); dex.chain, dex.default_chain — их читает сервер для суффикса.
-->
<script>
  import { get, seg } from '../lib/api.js';
  import { store } from '../lib/settings.svelte.js';
  import { checkDexId } from '../lib/dex.svelte.js';
  import { uniq } from '../lib/uniq.js';

  /** onadd(key) — добавить в карточку строку области «DEX» с этим ключом (значение — после «Сохранить»). */
  let { onadd } = $props();
  let raw = $state('');
  let picked = $state('');
  /** Причина отказа от сервера или по ключу целиком; '' — нет. */
  let error = $state('');
  let busy = $state(false);

  /** Площадки DEX шлюза по алфавиту (GET /api/venues/kinds). */
  const dexVenues = $derived(uniq(store.venueKinds.filter((x) => x.kind === 'Dex').map((x) => x.id)));
  /** Площадка нового ключа: одна площадка DEX — она, без выбора; несколько — выбранная. */
  const venue = $derived(dexVenues.length === 1 ? dexVenues[0] : picked);
  /** Причина, по которой введённый сырой id не подходит; '' — подходит или поле пустое. */
  const inputError = $derived(raw.trim() ? checkDexId(raw.trim()) : '');

  /**
   * Что делает: берёт у сервера суффикс сети площадки, строит ключ id + суффикс и отдаёт его карточке.
   * Ошибки: текст сервера (площадка не DEX, не задана dex.chain / dex.default_chain) или длина ключа — в error.
   */
  async function add() {
    error = '';
    busy = true;
    try {
      // Площадка и id — на момент нажатия; поля на время запроса заблокированы (busy), ключ не уйдёт чужой сети.
      const id = raw.trim();
      const at = venue;
      const r = await get(`/api/venues/${seg(at)}/dex-key-suffix`);
      const suffix = r?.suffix ?? '';
      const why = checkDexId(id, suffix);
      if (why) {
        error = why;
        return;
      }
      onadd(id + suffix);
      raw = '';
    } catch (e) {
      if (!e.aborted) error = e.message;
    } finally {
      busy = false;
    }
  }
</script>

<!-- Решение 128 / 490: DEX, которой нет в списке адаптера, — сырой id руками; суффикс сети — с сервера. -->
<div class="adder">
  {#if dexVenues.length > 1}
    <select bind:value={picked} disabled={busy} onchange={() => (error = '')}>
      <option value="">— площадка DEX —</option>
      {#each dexVenues as v (v)}<option value={v}>{v}</option>{/each}
    </select>
  {/if}
  <input
    bind:value={raw}
    disabled={busy}
    oninput={() => (error = '')}
    placeholder="id новой DEX, например lista-stable"
  />
  <button disabled={!raw.trim() || !!inputError || !venue || busy} onclick={add}>Добавить DEX</button>
  {#if inputError}<span class="err">{inputError}</span>{/if}
  {#if error}<span class="err">{error}</span>{/if}
</div>
{#if !dexVenues.length}
  <p class="muted">Площадок DEX в шлюзе нет — ключ области «DEX» добавить не для чего.</p>
{/if}
