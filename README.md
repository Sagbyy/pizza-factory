# Pizza Factory - Agent Distribue Rust Groupe 2 4AL1
Lien vers Git: https://github.com/Sagbyy/pizza-factory

## Bonus réalisés:
- Réaliser une interface graphique ou un dashboard (TUI/GUI) pour suivre l'état de la chaîne de production.
- Ajouter une intégration continue (CI) pour tester votre code automatiquement: (fmt/check/test/doc) sur GitHub Actions
- Réduire au maximum les unwrap(), expect() et les variables mutables (mut) ou les clones (clone()).
## 1. Objectif du projet

Ce projet implémente un agent et un client compatibles sur le thème production de pizzas.

Le système repose sur:

- un plan de controle UDP (gossip, presence, decouverte),
- un plan de donnees TCP (commandes client, forwarding inter-noeuds, reponses).

Le protocole a été reconstruit par reverse-engineering a partir de captures réseau et validé par des tests d'interoperabilité multi-noeuds.

## 2. Organisation d'équipe

- Membres:
  - Herman
  - Van Anh
  - Salahe-Eddine
- Répartition initiale:
  - Reverse-engineering et spécification
  - Services reseau UDP/TCP
  - Client CLI, tests, documentation
- Méthode de travail:
  - Branches courtes, revues de code, validation locale `cargo fmt`, `cargo check`, `cargo test`.

## 3. Demarche de reverse-engineering

### 3.1 Outils utilisés

- Wireshark/tcpdump pour capturer et inspecter les trames.
- Decodage CBOR pour identifier les structures de messages.

### 3.2 Hypothèses et validation

- UDP transporte les messages de decouverte et de presence (`Announce`, `Ping`, `Pong`).
- TCP transporte les commandes applicatives (`list_recipes`, `get_recipe`, `order`, payload process).
- Validation par captures pcap et execution de scénarios reproductibles.

Captures disponibles:

- `doc/pcap/starting-peer-annouced.pcap`
- `doc/pcap/client-command-2.pcapng`
- Frames sous forme yaml pour un flow UDP + un flow TCP de Wireshark. 

## 4. Architecture retenue

### 4.1 Vue globale

- Module `protocol`: modeles de messages + sérialisation/desérialisation CBOR.
- Module `network::udp`: boucle gossip, gestion presence et table des pairs.
- Module `network::tcp`: framing longueur-prefixée pour trames TCP.
- Module `server::tcp` + `server::handlers`: endpoint serveur et logique metier.
- Module `recipe`: parsing + flatten des etapes.
- Module `cli`: commandes `start`, `start-tui`, `client`.

### 4.2 Choix techniques

- Concurrence avec `std::thread` et `Arc/RwLock`.
- Pas de runtime async.
- `serde` + `ciborium` pour le format binaire.
- `clap` pour le CLI.
- `uuid` pour les identifiants de commandes.

## 5. Fonctionnalités implementées

### 5.1 Agent (serveur)

- Démarrage noeud avec capacités, peers bootstrap et fichier de recettes.
- Service gossip UDP:
  - annonces de capacités/recettes,
  - pings/pongs de présence,
  - mise a jour de l'état partage des pairs.
- Service TCP:
  - `ListRecipes`,
  - `GetRecipe`,
  - `Order`,
  - `ProcessPayload`.
- Forwarding inter-noeuds:
  - Si la recette/action n'est pas locale, le noeud sélectionne des peers annoncés via gossip et relaie la requete TCP.

### 5.2 Client

- Commandes CLI:
  - `list-recipes`
  - `get-recipe <RECIPE>`
  - `order <RECIPE>`
- Affichage des recettes locales et distantes connues.

## 6. Scénario de démonstration reproductible

Ouvrir 3 terminaux a la racine du projet.

Terminal 1:

```bash
cargo run -- start --host 127.0.0.1:8000 --capabilities MakeDough,Bake --recipes-file src/recipes/examples.recipes
```

Terminal 2:

```bash
cargo run -- start --host 127.0.0.1:8001 --capabilities Slice --peer 127.0.0.1:8000
```

Attendre 2 a 4 secondes (propagation gossip), puis Terminal 3:

```bash
cargo run -- client --peer 127.0.0.1:8001 list-recipes
cargo run -- client --peer 127.0.0.1:8001 get-recipe Margherita
cargo run -- client --peer 127.0.0.1:8001 order Margherita
```

Résultat attendu:

- 8001 connait les recettes annoncées par 8000,
- 8001 peut récuperer la recette et passer commande en relayant vers 8000,
- le client recoit la reponse finale depuis 8001.

## 7. Tests et qualite

- Formatage: `cargo fmt`
- Vérification compilation: `cargo check`
- Tests: `cargo test`
- Documentation: `cargo doc --open`

## 8. Chemins explorés et enseignements

### 8.1 Pistes non retenues / corrigees

- Première integration TCP/UDP avec blocage de boucle startup.
- Ajustements progressifs du forwarding (order puis get_recipe/payload).
- Ajustement de la boucle gossip pour eviter les blocages de reception.

### 8.2 Enseignements

- L'état partage gossip doit etre explicitement exploité par les handlers TCP.
- Les tests de framing et de bout en bout évitent les regressions silencieuses.
- Une documentation protocolaire explicite accélère les corrections d'interoperabilité.

## 9. Limites connues

- Politique de sélection de peer basique (premier peer qui repond).
- Gestion d'erreur reseau perfectible (retry/backoff avancé).

## 10. Conformite aux contraintes

- Langage Rust.
- Bibliotheque standard privilegiée.
- Concurrence via threads std et primitives std.
- Crates utilisees: `clap`, `serde`, `ciborium`, `uuid`.

## 11. Contributions individuelles

- Herman: modèles de données + gossip UDP + CI + log.
- Van Anh: Reverse-Engineering + Spec Protocole + Parseur de recette + handlers TCP.
- Salahe-Eddine: Reverse-Engineering + Spec Protocole + client CLI + client TUI + CI.

## 12. Annexes

- Reverse engineering: `REVERSE-ENGINEERING.md`
- Specification protocole: `PROTOCOLE-SPEC.md`
- Captures reseau: `doc/pcap/*`
- Commandes de test rapide: `src/cli/command.txt`
- Pipeline CI: (fmt/check/test/doc) sur GitHub Actions. La configuration des étapes la pipeline: `.github/workflows/rust.yml`
<img width="1903" height="988" alt="image" src="https://github.com/user-attachments/assets/4a02b5a8-49ba-40a5-b808-84dd11356f72" />

