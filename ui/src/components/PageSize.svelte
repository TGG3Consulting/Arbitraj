<!--
  Файл: ui/src/components/PageSize.svelte
  Что делает: поле «сколько записей показывать за раз». Значения по умолчанию нет: пусто — экран
    просит его задать. Выбор запоминается в браузере этого зрителя (lib/prefs.js).
  Кто вызывает: pages/Journal.svelte, components/SettingHistory.svelte.
  От чего зависит: lib/prefs.js.
  Настройки: нет (это удобство просмотра, не настройка системы).
-->
<script>
  import { untrack } from 'svelte';
  import { load, save } from '../lib/prefs.js';

  /** key — под каким именем запомнить; value — число записей или null (не задано); label — подпись. */
  let { key, value = $bindable(null), label } = $props();
  let text = $state(untrack(() => load(key)));

  $effect(() => {
    const n = /^\d+$/.test(text) && Number(text) > 0 ? Number(text) : null;
    value = n;
    if (n !== null) save(key, String(n));
  });
</script>

<label class="inline">
  {label}
  <input class="num" inputmode="numeric" bind:value={text} placeholder="не задано" />
</label>
