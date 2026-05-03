import { defineConfig } from 'vitepress'

// https://vitepress.dev/reference/site-config
export default defineConfig({
  title: 'cofferdam',
  description:
    "TypeScript code-quality analyzer — Rust core + JS plugin layer, inspired by Elixir's Credo",

  base: '/cofferdam/',

  lastUpdated: true,
  cleanUrls: true,

  themeConfig: {
    // https://vitepress.dev/reference/default-theme-config

    nav: [
      { text: 'Guide', link: '/' },
      { text: 'Checks', link: '/checks' },
      { text: 'Configuration', link: '/ignore' },
      { text: 'Reference', link: '/output-formats' },
      {
        text: 'GitHub',
        link: 'https://github.com/TAJD/cofferdam',
      },
    ],

    sidebar: [
      {
        text: 'Getting started',
        items: [{ text: 'Introduction', link: '/' }],
      },
      {
        text: 'Built-in checks',
        items: [{ text: 'Catalog', link: '/checks' }],
      },
      {
        text: 'Configuration',
        items: [
          { text: 'Ignoring files', link: '/ignore' },
          { text: 'Suppression directives', link: '/suppression' },
        ],
      },
      {
        text: 'Reference',
        items: [
          { text: 'Output formats', link: '/output-formats' },
          { text: 'CI recipes', link: '/ci-recipes' },
          { text: 'Doctor', link: '/doctor' },
        ],
      },
      {
        text: 'About',
        items: [
          {
            text: 'Contributing',
            link: 'https://github.com/TAJD/cofferdam/blob/main/CONTRIBUTING.md',
          },
        ],
      },
    ],

    socialLinks: [
      { icon: 'github', link: 'https://github.com/TAJD/cofferdam' },
    ],

    search: {
      provider: 'local',
    },

    editLink: {
      pattern: 'https://github.com/TAJD/cofferdam/edit/main/docs/:path',
      text: 'Edit this page on GitHub',
    },

    footer: {
      message: 'MIT License',
      copyright: 'Copyright © 2026 Thomas Dickson',
    },
  },
})
