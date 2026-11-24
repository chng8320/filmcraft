# This fork: FilmCraft for the Dataverse

This is the Dataverse's fork of [FilmCraft](https://github.com/storytold/filmcraft) by the ArtCraft team
(MIT OR Apache-2.0). The Dataverse runs its command line on the main server, headless, as the `filmcraft-engine`
package.

## Branches

| Branch | What it is |
|---|---|
| `main` | Exactly ArtCraft's `main`. Never commit here; it is only updated from them. |
| `dataverse` | `main` plus our own commits on top. This is what the Dataverse builds. |

Each of our changes is one commit on `dataverse`, kept small and in its own place, so it replays cleanly
on every new FilmCraft and can be offered back as a pull request. FilmCraft's rules for contributors
(`AGENTS.md`) apply to our commits too.

## Keeping up with FilmCraft

`.github/workflows/dataverse.yml` runs every Monday (and on demand from the Actions tab): it updates `main`
from ArtCraft, replays our commits on `dataverse` (opening an issue when one no longer applies), builds the
command line, runs its tests and attaches the program as `dataverse-film-cli` (Linux x86-64).

## Our changes

| Commit | What | Offered upstream? |
|---|---|---|
| (none yet) | | |

## Name

The ArtCraft name and logos are trademarks, not covered by the code licence; modified versions must not use
them. Our build is called `dataverse-film-cli` and is described as "based on FilmCraft by the ArtCraft team".
