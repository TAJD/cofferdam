import { defineConfig } from 'vitepress'
import { checksItems } from './sidebar-checks'

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
        items: [
          { text: 'Introduction', link: '/' },
          { text: 'Install', link: '/install' },
          { text: 'Language support', link: '/languages' },
        ],
      },
      { text: 'Built-in checks', collapsed: false, items: checksItems },
      {
        text: 'Configuration',
        items: [
          { text: 'Ignoring files', link: '/ignore' },
          { text: 'Suppression directives', link: '/suppression' },
          { text: 'Per-path overrides', link: '/overrides' },
          { text: 'Budgets & ratchet', link: '/budgets' },
          { text: 'Type-aware checks', link: '/type-aware-checks' },
        ],
      },
      {
        text: 'Reference',
        items: [
          { text: 'Output formats', link: '/output-formats' },
          { text: 'CI recipes', link: '/ci-recipes' },
          { text: 'Doctor', link: '/doctor' },
          { text: 'Agent advisory (advise)', link: '/reference/advise' },
        ],
      },
      {
        text: 'For agents',
        items: [
          { text: 'Agent onboarding', link: '/agents' },
          { text: 'Agent hooks', link: '/hooks' },
          { text: 'MCP server', link: '/mcp' },
          { text: 'Invariants', link: '/invariants' },
        ],
      },
      {
        text: 'Plugin SDK',
        items: [
          { text: 'Author guide', link: '/plugin-sdk-guide' },
          { text: 'End-to-end fixture contract', link: '/plugin-sdk-e2e' },
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
