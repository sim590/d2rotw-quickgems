# Contrat de plateforme

Tout le code qui dépend du système d'exploitation vit dans `src/platform/`.
Le reste du programme (`executor`, `vision`, `planner`, `config`) ne connaît
que les traits et les types de valeur définis dans `src/platform/mod.rs`.

Le backend est choisi à la compilation par `#[cfg(target_os)]`. Il n'y a ni
chargement dynamique, ni sélection au runtime.

## Les quatre contrats

| Trait            | Responsabilité                                                          |
|------------------|-------------------------------------------------------------------------|
| `WindowSystem`   | trouver la fenêtre du jeu, la mettre au premier plan, vérifier le focus |
| `ScreenCapture`  | capturer la zone cliente de la fenêtre                                  |
| `InputInjector`  | déplacer le pointeur, cliquer avec modificateurs, garde-fou du pointeur |
| `ControlSignals` | pause et arrêt d'urgence lus sur la plateforme                          |

`Platform` est un trait englobant, implémenté automatiquement pour tout type
qui implémente les quatre. L'exécuteur est générique (`&impl Platform`), donc
les appels sont résolus à la compilation.

Les raccourcis lus depuis le terminal (`F8`, `F9`, `Ctrl+C`) restent dans
`src/hotkeys.rs` : ils ne dépendent pas de la plateforme. L'exécuteur compose
les deux sources :

```rust
platform.take_pause() || hotkeys::take_pause()
```

## Types communs

- `Point` : coordonnées **écran globales**, `i32`. Les moniteurs à coordonnées
  négatives sont permis.
- `Size` : pixels **physiques**.
- `WindowInfo` : identifiant opaque, titre, `origin` (coin supérieur gauche de
  la **zone cliente**) et `size` (zone cliente en pixels physiques).
- `MouseButton` : `Left` ou `Right`.
- `Modifiers` : `NONE` ou `CTRL_SHIFT`.

## Garanties du contrat

### Capture

- Retourne la **zone cliente** de la fenêtre, sans le curseur, origine en haut
  à gauche.
- La taille doit correspondre à `WindowInfo.size`.
- Doit pouvoir être appelée en boucle : l'exécuteur attend les mises à jour de
  l'interface en relisant la même cellule.

### Focus

- `focus` met la fenêtre au premier plan et lui donne le focus clavier. C'est
  une opération au mieux : elle peut échouer ou être refusée par l'OS.
- `is_focused` est la source de vérité. L'exécuteur revérifie toujours avant
  d'agir et arrête le run si le focus est perdu.

### Clic

- `click(point, button, modifiers)` doit garantir que `modifiers` est actif
  **avant** l'appui du bouton et le reste **jusqu'après** son relâchement.
- Les délais internes au clic (maintien des modificateurs, maintien du bouton,
  attente après le relâchement, vérification de l'état des modificateurs)
  appartiennent au composant dorsal : ils font partie de la garantie du
  contrat et chaque plateforme doit pouvoir les ajuster.
- Le pointeur est déplacé au point demandé avant le clic.

### Garde-fou du pointeur

- `note_pointer` mémorise la position laissée par le programme.
- `pointer_moved_by_user` retourne vrai si la souris a bougé depuis cette
  position. C'est un arrêt d'urgence : l'exécuteur ne clique plus.

### Signaux de contrôle

- `emergency_requested` et `take_pause` sont interrogés par la boucle
  principale. Ils ne bloquent pas, mais peuvent pomper la file d'événements de
  la plateforme.
- Aucun rappel (callback) n'est utilisé : tout est interrogé.

### Erreurs

Toutes les méthodes retournent `anyhow::Result`. Une erreur de plateforme est
traitée comme fatale par l'exécuteur : arrêt, capture d'erreur, puis reprise
limitée au vidage du cube quand c'est possible.

## Temporisations

Deux catégories, à ne pas mélanger :

1. **Délais internes au clic** : dans le composant dorsal. Ils font partie de
   la garantie du contrat (modificateurs actifs, clic complet).
2. **Attentes d'orchestration** : dans `executor::Timing`, injecté par
   `RunOptions`.
   - `after_focus`, `transmute_wait`, `poll`, `park_settle`, `retry_pause`,
     `cell_timeout`, `cube_timeout`.
   - `Timing::real()` pour l'usage normal, `Timing::fast()` pour les tests.

L'exécuteur ne contient aucun `Duration::from_millis(...)` en dur.

## Ajouter une plateforme

1. Créer `src/platform/<os>.rs` avec un type qui possède un `new() -> Result<Self>`.
2. Implémenter `WindowSystem`, `ScreenCapture`, `InputInjector` et
   `ControlSignals` pour ce type.
3. Dans `src/platform/mod.rs` :

   ```rust
   #[cfg(target_os = "<os>")]
   mod <os>;

   #[cfg(target_os = "<os>")]
   pub use <os>::Type as NativePlatform;
   ```

4. Ajuster le `compile_error!` qui couvre les OS sans backend.
5. Documenter les particularités de la plateforme ci-dessous.
6. Ajouter des tests avec `MockPlatform` quand c'est possible.

Le type natif n'a pas besoin d'être générique ni object-safe : l'exécuteur
utilise `&impl Platform`.

## Tester sans matériel

- `src/platform/mock.rs` (derrière `#[cfg(test)]`) fournit des captures
  scriptées, enregistre les clics et simule le focus, la pause et l'arrêt
  d'urgence. Les tests de l'exécuteur l'utilisent avec `Timing::fast()`.
- Un backend Windows peut être testé sous Wine/Proton, dans le même préfixe
  que le jeu : Wine implémente `EnumWindows`, `SendInput` et GDI. Windows
  Graphics Capture n'est pas disponible sous Wine; prévoir un repli GDI
  (`PrintWindow`/`BitBlt`).

## Particularités connues

### Windows

- Déclarer le processus DPI-aware, sinon les coordonnées du profil sont
  décalées par la mise à l'échelle.
- Capture : Windows Graphics Capture pour le plein écran, avec repli
  `PrintWindow` ou `BitBlt`.
- Focus : `SetForegroundWindow` peut être refusé; utiliser
  `AttachThreadInput` puis revérifier avec `is_focused`.
- Entrée : `SendInput` doit envoyer les `keydown` avant le clic et les `keyup`
  après.
- Raccourcis : `RegisterHotKey` exige une boucle de messages;
  `GetAsyncKeyState` cadre mieux avec un contrat interrogé.

### macOS

- Capture : `ScreenCaptureKit`, avec les permissions « Enregistrement de
  l'écran » et « Accessibilité ».
- Entrée : `CGEvent`.
- Attention à l'origine des coordonnées (bas-gauche) et à l'échelle Retina.

### Wayland natif

- Capture via le portail PipeWire.
- L'injection d'entrée est généralement bloquée pour les clients non
  privilégiés.
- La voie supportée reste XWayland; documenter Wayland natif comme non
  supporté tant qu'il n'y a pas de backend dédié.

## Limites actuelles

- Un seul composant dorsal : X11/XWayland.
- Aucune sélection dynamique de backend.
- Pas de capture plein écran exclusif.
