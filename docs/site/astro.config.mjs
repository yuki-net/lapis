import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  integrations: [
    starlight({
      title: 'Lapis',
      description: 'Lapis のユーザー向けドキュメント',
      locales: { root: { label: '日本語', lang: 'ja' } },
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/yuki-net/lapis' },
      ],
      customCss: ['./src/styles/custom.css'],
      components: {
        Sidebar: './src/components/Sidebar.astro',
        Pagination: './src/components/Pagination.astro',
        PageTitle: './src/components/PageTitle.astro',
      },
      sidebar: [
        {
          label: 'はじめに',
          items: [
            { label: '概要', slug: 'index' },
            { autogenerate: { directory: 'guide' } },
          ],
        },
        {
          label: 'リファレンス',
          items: [{ autogenerate: { directory: 'reference' } }],
        },
      ],
    }),
  ],
});
