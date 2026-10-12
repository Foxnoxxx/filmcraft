# RECut — carnet de bord

RECut est la version personnalisée de [FilmCraft](https://github.com/storytold/filmcraft) (par l'équipe
ArtCraft et ses contributeurs, licence MIT OR Apache-2.0) utilisée par Rémi Corbeau (RECorbs) pour son
activité de monteur vidéo. Ce fichier est la mémoire du projet : chaque personnalisation y est notée,
avec les règles qui permettent de rester compatible avec FilmCraft.

**Pour un agent qui reprend le travail :** lire ce fichier en entier, puis `AGENTS.md` et `CLAUDE.md`,
dont les règles (zéro plantage, aucun asset Adobe, contrôles qualité) s'appliquent aussi à RECut.

## Installer et mettre à jour

- Chaque envoi sur `main` compile automatiquement une version Windows 64 bits
  (`.github/workflows/recut-windows.yml`) et la publie dans les **Releases** du dépôt :
  `RECut-<version FilmCraft>-b<numéro>-windows-x64.zip`.
- Dézipper où l'on veut et lancer `RECut.exe`. Pour mettre à jour, remplacer le dossier par celui de la
  nouvelle version : préférences, préréglages, journaux et récupération après plantage sont dans
  `%APPDATA%\RECut`, ils sont conservés.
- RECut et FilmCraft officiel (installé par le launcher ArtCraft) cohabitent : dossiers de données,
  nom de fenêtre et exécutable distincts.

## Règles

1. **Compatibilité des projets.** Le format `.fcproj` reste exactement celui de FilmCraft : on ne modifie
   jamais `crates/format` ni le sens des données existantes de `crates/project`. Les données propres à
   RECut vont dans un fichier à côté du projet (`<projet>.recut.json`), jamais dans le `.fcproj`.
   Ainsi FilmCraft officiel ouvre toujours les projets de RECut, et inversement, tant que les deux sont
   à la même version de schéma.
2. **Un projet = une version du logiciel.** Un projet enregistré par une version plus récente (schéma
   plus récent) est refusé par une version plus ancienne. Mettre RECut à jour avant d'ouvrir un projet
   enregistré par un FilmCraft plus récent.
3. **Archiver les montages livrés** en XML FCP7 ou OpenTimelineIO en plus du `.fcproj`.
4. **Marques.** La licence de marque ArtCraft (`docs/brand/LICENSE-brand.txt`) impose à une version
   modifiée de retirer les marques ArtCraft : RECut n'affiche ni le logo ni le nom ArtCraft comme sien.
   Il peut dire en texte simple qu'il est basé sur FilmCraft par l'équipe ArtCraft.
5. **Limiter les conflits avec FilmCraft.** Les ajouts de RECut vont de préférence dans de nouveaux
   fichiers ou de nouveaux modules. Quand un fichier de FilmCraft doit être modifié, la modification
   est la plus petite possible et marquée d'un commentaire `RECut:` (`git grep "RECut:"` les liste).
   Les noms de crates, de commandes et de fichiers de FilmCraft ne changent pas.
6. **Mêmes exigences que FilmCraft** : zéro plantage (`AGENTS.md` §0), contrôles qualité verts avant
   chaque envoi, un test pour chaque fonctionnalité.

## Mettre RECut à jour avec FilmCraft

```sh
git remote add upstream https://github.com/storytold/filmcraft   # une seule fois
git fetch upstream main
git merge upstream/main          # résoudre les conflits sur les lignes marquées RECut:
cargo xtask ci                   # ou au minimum fmt, clippy et les tests des crates touchées
git push origin main             # déclenche la compilation Windows
```

Si FilmCraft réintroduit des workflows dans `.github/workflows/`, les supprimer à nouveau (ils exigent
les secrets de signature d'ArtCraft).

## Personnalisations

| Date | Quoi | Fichiers |
|------|------|----------|
| 2026-10-12 | Nom RECut : fenêtre, état de fenêtre enregistré, boîtes de dialogue, journal, infos de l'exécutable Windows (`RECut.exe`) | `apps/filmcraft/src/main.rs`, `apps/filmcraft/build.rs` |
| 2026-10-12 | Dossier de données séparé (`%APPDATA%\RECut`, variable `RECUT_DATA_DIR`) | `crates/engine/src/autosave.rs` |
| 2026-10-12 | Logo RECut dessiné dans le code (point rouge d'enregistrement + « RECut ») à la place du logo ArtCraft ; en-tête : logo RECut à la place du bouton Discord ; À propos : nom, numéro de build, mention « basé sur FilmCraft », lien vers ce dépôt | `crates/ui-egui/src/brand.rs`, `header.rs`, `panels/dialogs.rs`, `tests/links_ui.rs` |
| 2026-10-12 | Compilation Windows automatique et publication en release ; suppression des workflows de FilmCraft | `.github/workflows/recut-windows.yml` |

## À faire

- Icône d'application RECut (aujourd'hui celle de FilmCraft) : à dessiner par Rémi, dans la DA RECorbs.
- Écran d'accueil et menu Aide : ils pointent encore vers la communauté ArtCraft (Discord, site).
- Traductions : les textes propres à RECut sont en français, sans passer par le catalogue de traduction.
