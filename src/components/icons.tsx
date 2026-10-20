//! 界面用的矢量图标：一律 24 格 viewBox、stroke 走 currentColor，
//! 这样明暗主题、hover 变色都只靠 CSS，不需要两套位图。

type IconProps = { size?: number };

function svgProps(size: number) {
  return {
    width: size,
    height: size,
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.7,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
    "aria-hidden": true,
  };
}

export function HomeIcon({ size = 20 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <path d="M3.6 10.8 12 4.2l8.4 6.6V19a1.6 1.6 0 0 1-1.6 1.6h-3.6v-6H8.8v6H5.2A1.6 1.6 0 0 1 3.6 19z" />
    </svg>
  );
}

export function ModsIcon({ size = 20 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <rect x="3.6" y="3.6" width="7" height="7" rx="1.6" />
      <rect x="13.4" y="3.6" width="7" height="7" rx="1.6" />
      <rect x="3.6" y="13.4" width="7" height="7" rx="1.6" />
      <path d="M13.4 16.9h7m-7 3.5h7m-3.5-7v7" />
    </svg>
  );
}

export function ToolsIcon({ size = 20 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <path d="M14.6 6.4a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.7-3.7a6 6 0 0 1-7.9 7.9l-6.8 6.8a2.1 2.1 0 0 1-3-3l6.8-6.8a6 6 0 0 1 7.9-7.9z" />
    </svg>
  );
}

/// GM 工具：魔法棒 + 两粒闪光。和「工具」的扳手、「开发者」的尖括号都不撞形。
export function GmIcon({ size = 20 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <path d="M4.6 19.4 12.4 11.6" />
      <path d="M17 3.6 18.2 5.8 20.4 7 18.2 8.2 17 10.4 15.8 8.2 13.6 7 15.8 5.8Z" />
      <path d="M7.4 3.4 7.9 5.1 9.6 5.6 7.9 6.1 7.4 7.8 6.9 6.1 5.2 5.6 6.9 5.1Z" />
    </svg>
  );
}

export function SavesIcon({ size = 20 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <ellipse cx="12" cy="6" rx="7.2" ry="2.8" />
      <path d="M4.8 6v12c0 1.55 3.22 2.8 7.2 2.8s7.2-1.25 7.2-2.8V6" />
      <path d="M4.8 12c0 1.55 3.22 2.8 7.2 2.8s7.2-1.25 7.2-2.8" />
    </svg>
  );
}

export function SettingsIcon({ size = 20 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <circle cx="12" cy="12" r="3.1" />
      <path d="M19.3 14.9a1.6 1.6 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-1.8-.3 1.6 1.6 0 0 0-1 1.5v.2a2 2 0 1 1-4 0v-.1a1.6 1.6 0 0 0-1-1.5 1.6 1.6 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.6 1.6 0 0 0 .3-1.8 1.6 1.6 0 0 0-1.5-1H2a2 2 0 1 1 0-4h.2a1.6 1.6 0 0 0 1.4-1.1 1.6 1.6 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.6 1.6 0 0 0 1.8.3H9a1.6 1.6 0 0 0 1-1.5V2a2 2 0 1 1 4 0v.2a1.6 1.6 0 0 0 1 1.5 1.6 1.6 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.6 1.6 0 0 0-.3 1.8V9a1.6 1.6 0 0 0 1.5 1h.2a2 2 0 1 1 0 4h-.1a1.6 1.6 0 0 0-1.5 1z" />
    </svg>
  );
}

export function DevIcon({ size = 20 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <path d="M8.4 17 3 12l5.4-5" />
      <path d="M15.6 7 21 12l-5.4 5" />
      <path d="M13.6 5.2 10.4 18.8" />
    </svg>
  );
}

export function FeedbackIcon({ size = 20 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <path d="M20.4 11.4v1.2a7.4 7.4 0 0 1-7.4 7.4H9.6L4.4 23v-4.4a7.4 7.4 0 0 1-1.8-4.8v-2.4" />
      <path d="M12 2.4a7.8 7.8 0 0 0-7.8 7.8v1.2a7.8 7.8 0 0 0 15.6 0v-1.2A7.8 7.8 0 0 0 12 2.4z" />
      <path d="M9.6 9.6a2.4 2.4 0 1 1 3.5 2.2c-.7.4-1.1.9-1.1 1.7" />
      <path d="M12 16.1v.1" />
    </svg>
  );
}

export function SunIcon({ size = 18 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2.6v2M12 19.4v2M4.6 12h-2M21.4 12h-2M6.7 6.7 5.3 5.3M18.7 18.7l-1.4-1.4M17.3 6.7l1.4-1.4M5.3 18.7l1.4-1.4" />
    </svg>
  );
}

export function MoonIcon({ size = 18 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <path d="M20 14.2A8.4 8.4 0 1 1 9.8 4a6.9 6.9 0 0 0 10.2 10.2z" />
    </svg>
  );
}

export function MinimizeIcon({ size = 18 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <path d="M5.5 12.5h13" />
    </svg>
  );
}

export function MaximizeIcon({ size = 18 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <rect x="5.2" y="5.2" width="13.6" height="13.6" rx="2.2" />
    </svg>
  );
}

export function RestoreIcon({ size = 18 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <rect x="4" y="8.2" width="11.8" height="11.8" rx="2.2" />
      <path d="M8.2 8.2V6a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-2.2" />
    </svg>
  );
}

export function CloseIcon({ size = 18 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <path d="M6 6l12 12M18 6 6 18" />
    </svg>
  );
}

/// 浮出：方框往右上角出去一个箭头。
export function DetachIcon({ size = 16 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <path d="M13.4 4.6h6v6" />
      <path d="M19.4 4.6 11.8 12.2" />
      <path d="M16.4 10.6v6.8a2 2 0 0 1-2 2H5.6a2 2 0 0 1-2-2V9.6a2 2 0 0 1 2-2h6.8" />
    </svg>
  );
}

/// 归位：箭头收进方框。
export function DockIcon({ size = 16 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <rect x="3.6" y="3.6" width="16.8" height="16.8" rx="2.4" />
      <path d="M12 7.6v6.6m0 0-3-3m3 3 3-3" />
      <path d="M7.6 16.4h8.8" />
    </svg>
  );
}

/// 收起成胶囊：箭头落进一枚胶囊里。
export function PillIcon({ size = 16 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <rect x="3.4" y="13.6" width="17.2" height="6.4" rx="3.2" />
      <path d="M12 3.2v7.6M9.2 8.2 12 11l2.8-2.8" />
    </svg>
  );
}

/// 抓手：提示这一段标题栏可以拖。
export function GripIcon({ size = 14 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <path d="M9 6.5h.01M9 12h.01M9 17.5h.01M15 6.5h.01M15 12h.01M15 17.5h.01" strokeWidth="2.4" />
    </svg>
  );
}

/// 垃圾桶：只在「这一步会少一份文件」的地方出现，配确认弹窗一起用。
export function TrashIcon({ size = 16 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <path d="M4.5 6.5h15M9.5 6.5V4.8A1.3 1.3 0 0 1 10.8 3.5h2.4a1.3 1.3 0 0 1 1.3 1.3v1.7" />
      <path d="M6.3 6.5 7.2 19a1.6 1.6 0 0 0 1.6 1.5h6.4A1.6 1.6 0 0 0 16.8 19l.9-12.5" />
      <path d="M10.3 10v6.6M13.7 10v6.6" />
    </svg>
  );
}

/// 账号胶囊上的头像：只画一个人形轮廓，配合胶囊本身表示「这是登录账号」。
export function UserIcon({ size = 15 }: IconProps) {
  return (
    <svg {...svgProps(size)}>
      <circle cx="12" cy="8.4" r="3.6" />
      <path d="M5.2 19.4c1.3-3.1 3.8-4.7 6.8-4.7s5.5 1.6 6.8 4.7" />
    </svg>
  );
}
