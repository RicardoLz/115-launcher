import { useEffect, useState } from "react";

export type Theme = "dark" | "light";
export type Scheme = "amber" | "violet" | "azure" | "teal" | "moss" | "rose";
export type Glass = "off" | "on";

const KEY = "launcher.theme";
const SCHEME_KEY = "launcher.scheme";
const GLASS_KEY = "launcher.glass";

/// 配色方案只管色相，明暗另算：CSS 里每条 :root[data-scheme=…] 只改两个变量。
export const SCHEMES: { key: Scheme; label: string }[] = [
  { key: "amber", label: "琥珀" },
  { key: "violet", label: "紫罗兰" },
  { key: "azure", label: "青蓝" },
  { key: "teal", label: "松石" },
  { key: "moss", label: "苔绿" },
  { key: "rose", label: "玫红" },
];

function read(): Theme {
  return window.localStorage.getItem(KEY) === "light" ? "light" : "dark";
}

function readScheme(): Scheme {
  const raw = window.localStorage.getItem(SCHEME_KEY);
  return SCHEMES.some((item) => item.key === raw) ? (raw as Scheme) : "amber";
}

/// 液态玻璃默认关：它要 GPU 合成一整层透光磨砂，且没有主页背景时材质根本看不出来，
/// 所以「原版」是缺省值，这枚开关是让玩家自己决定要不要为质感付这点开销。
function readGlass(): Glass {
  return window.localStorage.getItem(GLASS_KEY) === "on" ? "on" : "off";
}

/// 主题、配色与质感都存在 localStorage：主窗与浮出的面板窗口各自订阅，切换不用跨窗口传话。
export function useTheme() {
  const [theme, setTheme] = useState<Theme>(read);
  const [scheme, setScheme] = useState<Scheme>(readScheme);
  const [glass, setGlass] = useState<Glass>(readGlass);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    window.localStorage.setItem(KEY, theme);
  }, [theme]);

  useEffect(() => {
    document.documentElement.dataset.scheme = scheme;
    window.localStorage.setItem(SCHEME_KEY, scheme);
  }, [scheme]);

  useEffect(() => {
    document.documentElement.dataset.glass = glass;
    window.localStorage.setItem(GLASS_KEY, glass);
  }, [glass]);

  useEffect(() => {
    const sync = (event: StorageEvent) => {
      if (event.key === KEY) setTheme(event.newValue === "light" ? "light" : "dark");
      if (event.key === SCHEME_KEY && SCHEMES.some((item) => item.key === event.newValue)) {
        setScheme(event.newValue as Scheme);
      }
      if (event.key === GLASS_KEY) setGlass(event.newValue === "on" ? "on" : "off");
    };
    window.addEventListener("storage", sync);
    return () => window.removeEventListener("storage", sync);
  }, []);

  const toggleTheme = () => setTheme((prev) => (prev === "dark" ? "light" : "dark"));
  return { theme, toggleTheme, scheme, setScheme, glass, setGlass };
}
