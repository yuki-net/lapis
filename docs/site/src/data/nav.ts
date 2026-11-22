/**
 * 2 段ヘッダーのタブとサイドバーの対応表。
 * タブ = セクション、`group` = astro.config.mjs のサイドバーグループ名。
 */
export const sections = [
  {
    label: 'はじめに',
    href: '/guide/getting-started/',
    prefix: '/guide',
    group: 'はじめに',
  },
  {
    label: 'リファレンス',
    href: '/reference/settings/',
    prefix: '/reference',
    group: 'リファレンス',
  },
] as const;

export type Section = (typeof sections)[number];

export function currentSection(pathname: string): Section | undefined {
  return (
    sections.find((section) => pathname.startsWith(section.prefix)) ??
    (pathname === '/' ? sections[0] : undefined)
  );
}
