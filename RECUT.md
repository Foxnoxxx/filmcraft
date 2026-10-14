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

## Piloter RECut avec Claude

1. Lancer `RECut avec Claude.bat` (RECut écoute alors sur `127.0.0.1:9876`, en local uniquement).
2. Déclarer le pont MCP dans l'application Claude pour ordinateur :
   `{"mcpServers": {"recut": {"command": "<dossier RECut>\\filmcraft-cli.exe", "args": ["mcp", "--bridge", "127.0.0.1:9876"]}}}`.
3. Dans une conversation liée à l'ordinateur, Claude voit les outils de RECut (commandes, clics,
   captures du moniteur Programme) et peut monter en direct. Détails : `docs/agents.md`.

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
| 2026-10-13 | **Imbriquer en multicaméra** (`recut.nestMulticam`, menu Clip et clic droit) : des clips déjà synchronisés, empilés sur plusieurs pistes vidéo, deviennent une séquence source multicaméra (une caméra par piste vidéo, avec le son qui lui est lié) remplacée par un clip multicaméra dont le son suit l'angle. Le son lié à aucune image (musique) reste audible sur tous les angles | `crates/engine/src/recut.rs`, `recut_tests.rs`, `commands.rs`, `lib.rs`, `ui-egui/src/panels/timeline.rs`, catalogues `i18n/*.tsv` |
| 2026-10-13 | Créer une séquence source multicaméra : chaque caméra garde **toutes ses pistes audio** (tous les flux d'un enregistrement OBS, et les clips de Canaux audio), au lieu du seul premier flux ; mode audio « Changer l'audio » par défaut dans la boîte de dialogue | `crates/engine/src/multicam.rs`, `ui-egui/src/state.rs` |
| 2026-10-13 | « L'audio multicaméra suit la vidéo » activé par défaut (désactivé dans FilmCraft et Premiere) | `crates/engine/src/lib.rs`, `multicam_tests.rs` (3 tests adaptés) |
| 2026-10-13 | Clic droit sur un clip : **Gain audio…** (dont normalisation) et **Canaux audio…** ; depuis la timeline, Canaux audio modifie le clip de la timeline (une seule voie cochée = mono des deux côtés), même si un élément du panneau Projet est sélectionné | `ui-egui/src/panels/timeline.rs`, `panels/clip_dialogs.rs`, `tests/recut_ui.rs` |
| 2026-10-13 | **Recherche de police** : champ de recherche en tête de la liste des polices (Graphiques), qui prend le clavier à l'ouverture | `ui-egui/src/panels/graphics.rs`, `tests/recut_ui.rs` |
| 2026-10-13 | Formes d'onde audio : au plus 2 flux analysés à la fois, analyse en pause pendant la lecture, avancement affiché dans la barre d'état (avant : tous les flux décodés en même temps, lecture saccadée sur de longs rushs) | `ui-egui/src/panels/timeline.rs`, `ui-egui/src/lib.rs` |
| 2026-10-14 | **Équilibrer le son multicam…** (`recut.balanceMulticam`, clic droit sur un clip multicam) : mesure chaque piste son de la source (LUFS échantillonnés, sans le gain de clip), repère les pistes « mixage OBS » (piste = somme des autres, par moindres carrés) et les pistes vides, propose un rôle par piste (voix = la plus faible d'une caméra, jeu, voix + jeu, coupée, inchangée) ; applique : gain de clip vers la cible (voix -20 LUFS, jeu 8 dB dessous, voix + jeu à mi-chemin), passe-haut 80 Hz sur la voix, compresseur par piste, limiteur -1 dB sur le mixage de la source. Les effets gérés sont mis à jour, pas empilés. Boîte avec analyse en arrière-plan | `crates/engine/src/recut_balance.rs`, `recut_tests.rs`, `ui-egui/src/panels/recut_balance.rs`, `menus.rs`, `panels/dialogs.rs`, `panels/timeline.rs`, `tests/recut_ui.rs`, catalogues `i18n/*.tsv` |
| 2026-10-14 | Imbriquer en multicaméra : position du Mouvement en nombres (Propriétés affichait « NaN ») ; pistes son nommées d'après la caméra (« zenh 1 », « zenh 2 ») | `crates/engine/src/recut.rs` |
| 2026-10-14 | **Appliquer les habillages des POV** (`recut.povOverlays`, clic droit sur le clip multicam) : l'habillage posé à la main sur la première tranche de chaque caméra (pistes vidéo au-dessus du multicam) est le modèle ; il est copié sur toutes les autres tranches de la caméra (étiré s'il couvrait toute la tranche, sinon même décalage et même durée). Relancer après un nouveau découpage remplace les copies ; un clip modifié par l'utilisateur, le modèle et les clips en place ne sont jamais touchés | `crates/engine/src/recut_overlays.rs`, `recut_tests.rs`, `ui-egui/src/panels/timeline.rs`, `panels/recut_balance.rs`, `tests/recut_ui.rs`, catalogues `i18n/*.tsv` |
| 2026-10-14 | `RECut avec Claude.bat` dans le zip : lance RECut avec son canal de contrôle (port 9876) pour que Claude le pilote via le pont MCP | `.github/workflows/recut-windows.yml` |
| 2026-10-14 | **Temps morts** : Séquence ▸ **Marquer les temps morts…** (`recut.markDeadTime`, aussi au clic droit sur le clip multicam) écoute les pistes de voix de la source multicam (ou les pistes audio de la séquence) et repère les passages où personne ne parle (≥ 3 s, 0,5 s gardée après la parole, 0,3 s avant) ; la boîte de dialogue mesure une fois en arrière-plan (plusieurs pistes à la fois), puis chaque réglage et chaque piste cochée recalcule tout de suite ; un clic sur un temps mort y place la tête de lecture ; « Poser les marqueurs » pose un marqueur rose « Temps mort » par passage, dans le montage (à travers les clips multicam). Séquence ▸ **Supprimer les temps morts marqués** (`recut.removeDeadTime`) supprime avec raccord ce que couvrent les marqueurs restants. Seuils réglés sur les enregistrements OBS du 2026-05-20 | `crates/engine/src/recut_deadtime.rs`, `recut.rs`, `recut_overlays.rs`, `recut_tests.rs`, `lib.rs`, `ui-egui/src/panels/recut_deadtime.rs`, `menus.rs`, `panels/dialogs.rs`, `panels/timeline.rs`, `tests/recut_ui.rs`, catalogues `i18n/*.tsv` |
| 2026-10-14 | **Dossier de veille** : Fichier ▸ **Dossier de veille…** (`recut.watchFolder`) : RECut surveille un dossier (et ses sous-dossiers) et importe chaque nouveau média dès qu'il est complet (taille et date inchangées depuis 10 s, sur deux passages), dans un chutier au nom de son sous-dossier (dossier SwissTransfer, dossier du projet), sinon « Rushs <date OBS> » ; les fichiers déjà là au démarrage de la veille et ceux déjà dans le projet sont laissés de côté. Réglage gardé dans `recut-watch.json` (dossier de données RECut) ; recherche en arrière-plan toutes les 3 s | `crates/engine/src/recut_watch.rs`, `lib.rs`, `recut_tests.rs`, `ui-egui/src/panels/recut_watch.rs`, `menus.rs`, `lib.rs`, `panels/dialogs.rs`, `tests/recut_ui.rs`, catalogues `i18n/*.tsv` |
| 2026-10-14 | Clic droit sur un clip : les commandes RECut (Imbriquer en multicaméra, Équilibrer le son, Habillages des POV, Temps morts) sont regroupées dans un sous-menu **RECut** : le menu, devenu plus haut que l'écran sur une fenêtre de moins de 1000 px, tient de nouveau (« Afficher dans le projet », en bas, était inaccessible). L'indication « Analyse audio des formes d'onde » n'est plus un élément d'automatisation (elle faussait la mesure de la zone des panneaux) | `ui-egui/src/panels/timeline.rs`, `ui-egui/src/lib.rs`, `tests/recut_ui.rs`, catalogues `i18n/*.tsv` |

## À faire

- **Fenêtres sur le 2e écran** : les panneaux flottants restent dans la fenêtre principale (limite connue de
  FilmCraft, ticket #466, estimée à 20–35 h). À traiter quand on aura les retours sur le reste.
- **Textes qui débordent** sur certains paramètres : à repérer panneau par panneau (captures de Rémi).
- **Indicateur de chargement** des longs rushs au-delà des formes d'onde (index, miniatures) si la lenteur persiste.
- Synchronisation par l'heure de début des fichiers OBS (le nom de fichier contient l'heure, lu par
  `recut_watch::obs_time`) puis affinage audio dans une fenêtre de ±60 s : multicam prête dès l'arrivée des POV
  dans le dossier de veille.
- **Sous-titres** : FilmCraft sait transcrire (Whisper, en pur Rust) et créer des sous-titres, mais la fonction
  n'est pas compilée dans RECut (option `whisper`) et la transcription bloque l'interface pendant tout le calcul :
  à passer en tâche de fond avec progression avant de l'activer.
- **Dérush assisté** : repérer les moments forts (rires, cris, pics de voix simultanés, mots-clés de la
  transcription) et les marquer, sur le modèle des temps morts.
- Tests de FilmCraft qui échouent ici pour des raisons d'environnement (pas de RECut) : oracles ffmpeg (APV,
  Matroska, EXR, préréglages d'export, `workflows`) avec le ffmpeg 6.1 du conteneur, et tests GPU (`fx_tests`)
  sans carte graphique.
- Un test de FilmCraft échoue déjà sans les modifications de RECut :
  `nest_editing_tests::with_the_toggle_off_a_sequence_edits_in_as_its_clips_with_their_transitions`.

- Icône d'application RECut (aujourd'hui celle de FilmCraft) : à dessiner par Rémi, dans la DA RECorbs.
- Écran d'accueil et menu Aide : ils pointent encore vers la communauté ArtCraft (Discord, site).
- Traductions : les textes propres à RECut sont en français, sans passer par le catalogue de traduction.
