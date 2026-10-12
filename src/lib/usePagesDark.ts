import { useSyncExternalStore } from "react";
import { useStore } from "../store";
import { pagesAreDark } from "./pageDark";
import { usePlates } from "./platesView";

const query = () => window.matchMedia("(prefers-color-scheme: dark)");

const subscribe = (cb: () => void) => {
  const q = query();
  q.addEventListener("change", cb);
  return () => q.removeEventListener("change", cb);
};

/** Whether the system's own colour scheme is dark right now (the app's "match system"). */
export function useSystemDark(): boolean {
  return useSyncExternalStore(subscribe, () => query().matches);
}

/** Whether the pages show dark: the setting resolved against the app theme. Never in Output Preview: inverted inks are the wrong inks. */
export function usePagesDark(): boolean {
  const setting = useStore((s) => s.pageDark);
  const theme = useStore((s) => s.theme);
  const systemDark = useSystemDark();
  const plates = usePlates((s) => s.view !== null);
  return !plates && pagesAreDark(setting, theme, systemDark);
}
