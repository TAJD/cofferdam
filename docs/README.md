# cofferdam docs site

Built with [VitePress](https://vitepress.dev). Deployed to GitHub Pages at <https://tajd.github.io/cofferdam>.

## Local development

From the `docs/` directory:

```bash
pnpm install
pnpm docs:dev    # http://localhost:5173/cofferdam/
```

## Deploy

Pushes to `main` that touch `docs/**` or `packages/cofferdam/README.md` automatically build and deploy via `.github/workflows/docs.yml`. The deploy uses GitHub Actions — make sure repo Settings → Pages → Source is set to "GitHub Actions" (one-time setting; if you see a 404 after first deploy, check this).
