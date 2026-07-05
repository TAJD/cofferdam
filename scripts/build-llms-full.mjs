#!/usr/bin/env node
// scripts/build-llms-full.mjs — concatenate every docs page's raw markdown
// source into docs/public/llms-full.txt (CD-63 E2).
//
// VitePress serves pre-rendered HTML, which is fine for browsers but means
// an agent that just wants prose has to strip Vue-rendered markup per page.
// llms-full.txt is the llmstxt.org convention for "the whole corpus, one
// fetch" -- cheaper than crawling every page individually. Runs before
// `vitepress build` (see package.json `docs:build`) so the file exists in
// `public/` in time to be copied into `dist/` at the site root.

import { readdirSync, readFileSync, statSync, mkdirSync, writeFileSync } from 'node:fs'
import { join, relative, extname, basename, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const DOCS_ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', 'docs')
const OUT_PATH = join(DOCS_ROOT, 'public', 'llms-full.txt')

// Not part of the VitePress site (not referenced from config.ts nav) or
// not hand-authored source -- skip when walking for pages to mirror.
const SKIP_DIRS = new Set(['.vitepress', 'node_modules', 'public', 'superpowers'])

function walk(dir) {
  const out = []
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry)
    const stat = statSync(path)
    if (stat.isDirectory()) {
      if (SKIP_DIRS.has(entry)) continue
      out.push(...walk(path))
    } else if (extname(entry) === '.md') {
      out.push(path)
    }
  }
  return out
}

// Mirrors VitePress `cleanUrls: true`: strip `.md`, `index` collapses to
// the directory itself (trailing slash), root `index.md` becomes `/`.
function siteUrl(absPath) {
  const rel = relative(DOCS_ROOT, absPath).replace(/\\/g, '/')
  if (rel === 'index.md') return '/'
  if (basename(rel) === 'index.md') return `/${dirname(rel)}/`
  return `/${rel.slice(0, -'.md'.length)}`
}

function stripFrontmatter(content) {
  if (!content.startsWith('---\n') && !content.startsWith('---\r\n')) return content
  const end = content.indexOf('\n---', 4)
  if (end === -1) return content
  const afterDashes = content.indexOf('\n', end + 1)
  return afterDashes === -1 ? '' : content.slice(afterDashes + 1)
}

const files = walk(DOCS_ROOT).sort()
const sections = files.map((path) => {
  const body = stripFrontmatter(readFileSync(path, 'utf8')).trimEnd()
  return `<!-- page: ${siteUrl(path)} -->\n\n${body}\n`
})

mkdirSync(dirname(OUT_PATH), { recursive: true })
writeFileSync(OUT_PATH, sections.join('\n---\n\n'))
process.stdout.write(`wrote ${OUT_PATH} (${files.length} pages)\n`)
