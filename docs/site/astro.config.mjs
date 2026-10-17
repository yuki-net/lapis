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
      sidebar: [
        { slug: 'index' },
        { label: 'ガイド', items: [{ autogenerate: { directory: 'guide' } }] },
      ],
    }),
  ],
});
