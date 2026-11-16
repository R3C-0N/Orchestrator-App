<script lang="ts">
  // Interruption (`08`, `c4`, `k2`) : « ⎿  Interrupted · What should Claude
  // do instead? » en gris (pas en rouge), sous la ligne précédente.
  import type { InterruptedRow } from "../../state/session.svelte";
  import ToolResult from "./ToolResult.svelte";

  let { row }: { row: InterruptedRow } = $props();
</script>

<ToolResult lines={[row.text]} tone="dim" />
