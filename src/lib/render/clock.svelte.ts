// Horloge partagée pour les durées vivantes (« Running … · 3s », indicateur
// de tour). Elle ne tourne que si une ligne affichée la lit : les lignes
// démontées par la liste virtualisée ne coûtent rien.

import { createSubscriber } from "svelte/reactivity";

class Ticker {
  #period: number;
  #subscribe: () => void;

  constructor(period: number) {
    this.#period = period;
    this.#subscribe = createSubscriber((update) => {
      const id = setInterval(update, this.#period);
      return () => clearInterval(id);
    });
  }

  /** `Date.now()`, relu à chaque période par les lecteurs réactifs. */
  get now(): number {
    this.#subscribe();
    return Date.now();
  }
}

/** Une mise à jour par seconde (durées). */
export const seconds = new Ticker(1000);
/** Une mise à jour toutes les 120 ms (glyphe animé de l'indicateur). */
export const frames = new Ticker(120);
