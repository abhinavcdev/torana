# torana site

The marketing site and documentation at https://abhinavcdev.github.io/torana/, built with [Astro](https://astro.build) as static, per-page-indexable HTML for SEO (no client-side routing) with a light/dark theme toggle.

## Develop

```bash
npm install
npm run dev       # http://localhost:4321/torana/
npm run build     # outputs to dist/
npm run preview   # serve the production build locally
```

Deploys automatically to GitHub Pages on every push to `main` that touches `web/` (see `.github/workflows/deploy-site.yml`).

## Structure

- `src/pages/index.astro`: the landing page (hero, canvas traffic simulation, benchmarks, use cases, install snippets)
- `src/pages/docs/*.astro`: one real route per docs topic, each with its own title/description for search indexing
- `src/layouts/`: `BaseLayout` (SEO meta, theme init script, header/footer) and `DocsLayout` (adds the docs sidebar)
- `src/components/`: `Header` (includes the theme toggle), `Footer`, `CodeBlock` (copy-to-clipboard), `PhysicsHero` (canvas simulation)
- `src/styles/global.css`: the whole design system as CSS custom properties, redefined under `prefers-color-scheme` and overridden by `[data-theme]` when the user has explicitly toggled

## Theme toggle

Defaults to the OS preference (`prefers-color-scheme`). Clicking the toggle sets `data-theme` on `<html>` and persists the choice to `localStorage`; a blocking inline script in `<head>` applies a stored preference before first paint to avoid a flash of the wrong theme.
