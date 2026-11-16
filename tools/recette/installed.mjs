// Recette J12 : l'app installée (NSIS, dossier de test) démarre.
//   node tools/recette/installed.mjs <chemin de orchestrator.exe installé>
import { join } from "node:path";
import { WORK, closeApp, launchApp, log, save, sleep, tree, visibleWindows, waitGone } from "./lib.mjs";

const exe = process.argv[2];
const app = await launchApp({ exe });
await app.page.waitForSelector('[data-testid="project-picker"]', { timeout: 20_000 });
await sleep(1500);
const res = {
  exe,
  url: app.page.url(),
  cdpMs: app.tCdp - app.tSpawn,
  pickerRows: await app.page.getByTestId("project-row").count(),
  windows: visibleWindows(tree(app.pid).map((p) => p.pid)).map((w) => ({ cls: w.cls, title: w.title })),
};
try {
  await app.page.screenshot({ path: join(WORK, "installe-demarre.png"), timeout: 15_000 });
  res.shot = "installe-demarre.png";
} catch (e) {
  res.shot = `échec : ${e.message.split("\n")[0]}`;
}
await app.browser.close().catch(() => {});
res.close = closeApp(app.pid);
res.gone = (await waitGone([app.pid], 10_000)) ? "ok" : "restant";
log(JSON.stringify(res));
save("installe.json", res);
