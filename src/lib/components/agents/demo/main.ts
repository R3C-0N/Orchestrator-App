// Point d'entrée de la page de démonstration de la colonne des agents (J9).
import { mount } from "svelte";
import "../../../../app.css";
import AgentDemo from "./AgentDemo.svelte";

const target = document.getElementById("app");
if (!target) throw new Error("#app introuvable");
export default mount(AgentDemo, { target });
