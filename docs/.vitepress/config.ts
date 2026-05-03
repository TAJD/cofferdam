import { defineConfig } from 'vitepress'

// Static sidebar for Built-in checks.
// import.meta.glob is not available in VitePress config (Node ESM context,
// not inside a Vite transform). Maintain this list in parallel with
// docs/checks/ when new checks are generated via `cofferdam gen-docs`.
// Categories: Consistency → Design → Readability → Refactor → Warning.
const checksItems = [
  { text: 'All checks', link: '/checks/' },
  {
    text: 'Consistency',
    collapsed: true,
    items: [
      { text: 'QuoteStyle', link: '/checks/Consistency.QuoteStyle' },
    ],
  },
  {
    text: 'Design',
    collapsed: true,
    items: [
      { text: 'DuplicateExportName', link: '/checks/Design.DuplicateExportName' },
      { text: 'MaxParameters', link: '/checks/Design.MaxParameters' },
    ],
  },
  {
    text: 'Readability',
    collapsed: true,
    items: [
      { text: 'MaxFunctionLength', link: '/checks/Readability.MaxFunctionLength' },
      { text: 'MaxLineLength', link: '/checks/Readability.MaxLineLength' },
    ],
  },
  {
    text: 'Refactor',
    collapsed: true,
    items: [
      { text: 'CognitiveComplexity', link: '/checks/Refactor.CognitiveComplexity' },
      { text: 'CyclomaticComplexity', link: '/checks/Refactor.CyclomaticComplexity' },
      { text: 'DuplicateBlock', link: '/checks/Refactor.DuplicateBlock' },
      { text: 'PreferNullishCoalescing', link: '/checks/Refactor.PreferNullishCoalescing' },
      { text: 'PreferOptionalChain', link: '/checks/Refactor.PreferOptionalChain' },
      { text: 'UnusedVariable', link: '/checks/Refactor.UnusedVariable' },
    ],
  },
  {
    text: 'Warning',
    collapsed: true,
    items: [
      { text: 'NoConsoleLog', link: '/checks/Warning.NoConsoleLog' },
      { text: 'NoDebugger', link: '/checks/Warning.NoDebugger' },
      { text: 'NoEval', link: '/checks/Warning.NoEval' },
      { text: 'TripleEquals', link: '/checks/Warning.TripleEquals' },
    ],
  },
]

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
      { text: 'Built-in checks', collapsed: false, items: checksItems },
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
