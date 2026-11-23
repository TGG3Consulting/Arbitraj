<!-- Пояснение расчёта объёма в Live и Deals. Старые планы сохраняют текст и все причины пересчёта. -->
<script>
  import { explainSize } from '../lib/sizeExplanation.js';

  let { size } = $props();
  let text = $derived(explainSize(size?.why));
</script>

{#if text}
  <p class="muted">{text}</p>
  {#if text !== size.why}
    <details><summary>Исходное пояснение подбора</summary><p>{size.why}</p></details>
  {/if}
{/if}
