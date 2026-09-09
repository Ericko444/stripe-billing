# Revue du livrable face au cahier des charges

Revue du dépôt `stripe-billing` à l'état de `master` (`bda0697`), confrontée
au cahier des charges du challenge technique.

Les constats ci-dessous sont **vérifiés mécaniquement**, pas déduits de la
documentation du dépôt. Les commandes utilisées sont données pour que chaque
chiffre soit refaisable.

---

## 1. État des gates, à la date de la revue

| Gate | Commande | Résultat |
|---|---|---|
| Frontière S1 | `grep -E "sqlx\|axum\|stripe" crates/domain/Cargo.toml` | aucune sortie |
| Formatage | `cargo fmt --all --check` | vert |
| Lints | `cargo clippy --workspace --all-targets` | vert, 0 warning |
| Tests Rust | `cargo test --workspace` | **305 tests, 0 échec** |
| Typage front | `npm run typecheck` | vert |
| Tests front | `npx vitest run` | **8 tests, 0 échec** |

Volumétrie : ~20 700 lignes de Rust sur 6 crates, ~1 700 lignes de TS/TSX,
13 migrations SQL.

> **Note d'environnement (Windows / Docker Desktop).** Les tests
> `testcontainers` échouent en `WaitContainer(StartupTimeout)` si le contexte
> Docker actif est `desktop-linux` sans `DOCKER_HOST` explicite : bollard
> retombe alors sur `npipe:////./pipe/docker_engine`, qui n'existe pas. Avec
> `DOCKER_HOST='npipe:////./pipe/dockerDesktopLinuxEngine'`, la suite passe
> intégralement. Ce n'est pas un défaut du code, mais cela mérite une ligne
> dans le README : c'est exactement le genre de friction qui coûte cher en
> démonstration live.

---

## 2. Note globale : 90 / 100

| Axe | Note | Commentaire |
|---|---|---|
| Standards de code backend | 19/20 | Tous les points exigés vérifiés mécaniquement |
| Architecture / séparation des crates | 20/20 | Au-dessus de l'attendu |
| Webhooks + idempotence | 20/20 | Nettement au-dessus de l'attendu |
| Périmètre fonctionnel backend | 15/20 | Deux manques réels |
| Frontend | 16/20 | Propre et correct, mais mince |
| Livrable (README, doc, CI) | 13/20 | Excellent README, doc cassée, pas de CI |

---

## 3. Ce qui est au-dessus du cahier des charges

### L'idempotence

Ce n'est pas un `Uuid::new_v4()` posé en en-tête au moment de l'appel. C'est un
registre persistant indexé par `(tenant, opération, empreinte)`, avec une
machine à six états, la clé **écrite en base avant l'appel** et complétée
après, et une fenêtre de 23 h calée juste sous les 24 h de rétention de Stripe.

Trois détails qui font la différence :

- L'empreinte est **préfixée en longueur**, donc deux découpages de champs
  différents ne peuvent pas produire la même valeur.
- Le cas « ligne incomplète dont la clé a expiré » ne devine pas : il renvoie
  une erreur typée nommant la réconciliation. Réutiliser la clé ne déduplique
  plus chez Stripe ; en forger une neuve risque un doublon si l'appel initial
  a effectivement abouti. Ne rien faire est la seule réponse correcte, et
  c'est celle qui est implémentée.
- La perte de course à l'insertion relit la clé du gagnant au lieu d'échouer.

L'argument central — « une clé générée par tentative est une clé *neuve* au
retry, donc Stripe voit une requête sans rapport et crée un second
abonnement » — est le bon, et il est appliqué, pas seulement énoncé.

### Les webhooks

Le corps est lu en `Bytes` bruts, jamais en `Json<T>` : la signature est un
HMAC sur les octets exacts envoyés, et un extracteur qui désérialise puis
ré-encode casse la vérification irrémédiablement. La vérification précède tout
parsing.

- La signature est acceptée si **n'importe laquelle** des valeurs `v1` de
  l'en-tête correspond. C'est indispensable pendant une rotation de secret, où
  Stripe envoie une signature par secret actif — ne vérifier que la dernière
  rejette des livraisons valides pendant toute la durée de la rotation. Un
  test nommé épingle ce comportement.
- Tolérance de rejeu de 5 minutes, **symétrique** : un horodatage trop dans le
  futur est aussi suspect qu'un horodatage trop ancien.
- La déduplication **est** l'insertion : chaque identifiant d'événement heurte
  une contrainte unique et le conflit constitue le signal. Il ne reste aucune
  fenêtre dans laquelle une redélivrance pourrait se glisser, contrairement à
  un « check puis insert ».
- L'ordre est résolu par Postgres : chaque ligne du miroir porte le
  `created_at` de l'événement et chaque mise à jour est gardée par
  `last_event_created_at <= EXCLUDED.last_event_created_at`. Un événement
  arrivé en retard ne s'applique tout simplement pas.
- Le tenant est résolu depuis le miroir par `stripe_customer_id`, **jamais lu
  du payload**.

Neuf types d'événements sont traités, couvrant les quatre familles demandées ;
tout le reste est acquitté et ignoré, en 200 — un 500 sur « on ne gère pas ce
type » mettrait Stripe en file de retry permanente contre un handler qui ne
peut jamais réussir.

### Les frontières architecturales

`domain/Cargo.toml` ne contient ni `sqlx`, ni client Stripe, ni `axum` : c'est
la preuve, vérifiable en une commande, de la séparation. `api` expose un
`Router` et jamais un binaire, générique sur un `TenantExtractor` fourni par
l'hôte — le tenant **ne traverse jamais le fil en entrée**, et c'est une
propriété de compilation, pas une convention documentée.

`service` ne dépend ni de `persistence` ni de `stripe-adapter`, ce qui est
précisément pourquoi ses 70 tests — y compris le processeur de webhooks
complet — tournent Docker éteint, contre des doubles.

---

## 4. Conformité aux standards exigés

| Exigence du cahier des charges | Constat |
|---|---|
| Workspace multi-crates, pas un monolithe | 6 crates, dépendances strictement entrantes |
| `sqlx` en requêtes runtime uniquement | **0 occurrence** de `query!` / `query_as!` / `query_scalar!` |
| Placeholders `$N` | `$1` … `$9`, aucun autre style |
| Migrations numérotées et idempotentes | 13 migrations, `IF NOT EXISTS` sur chacune |
| `tenant_id` sur toute entité + soft-delete | 4 couches d'application ; index **partiels** `WHERE deleted_at IS NULL` et `ON DELETE RESTRICT` |
| Montants en entiers + devise, jamais de flottant | **0 `f64` / `f32`** dans tout le workspace *et* le front |
| Erreurs typées, pas d'`unwrap()` / `panic!` | `unwrap_used`, `expect_used`, `panic` en `deny` **workspace**, **0 `#[allow]`**, 0 occurrence en code de production |
| Tests d'intégration contre une vraie base | `testcontainers`, aucun mock de DB |
| Secrets par variables d'environnement | `SecretString` dès la lecture ; un test asserte qu'un `client_secret` n'atteint aucune ligne de log ; aucun secret en URL |
| Rustdoc sur l'API publique + README | `missing_docs = warn` workspace ; README de 342 lignes argumentant chaque choix |
| Front : client typé, cache, pas de `fetch` dispersé | TanStack Query ; **un seul `fetch`** dans toute l'application |
| Front : composants découpés, états explicites | 6 composants, hooks séparés, `host/` n'importe rien de `billing/` |
| Front : aucune clé secrète côté client | seule `VITE_STRIPE_PUBLISHABLE_KEY` ; le README donne la commande de vérification sur le bundle |

Le point sur le multi-tenant mérite d'être souligné : l'isolation est enforcée
à quatre niveaux indépendants — le type (un newtype, pas un `Uuid` nu), la
signature du port (aucune méthode ne *peut* lire à travers les tenants), le
SQL (`WHERE tenant_id = $1`), et les tests (un accès inter-tenant renvoie 404,
la même réponse qu'un identifiant inconnu, de sorte que l'API ne peut pas
servir à sonder l'existence des enregistrements d'un autre tenant).

Le rejet argumenté de RLS est solide : avec un `PgPool` partagé, RLS
impliquerait un `SET LOCAL` sur chaque transaction et la certitude qu'aucune
requête n'y échappe — on échangerait un prédicat greppable contre une
propriété de session implicite dont le mode de défaillance est une lecture
silencieuse de toute la table.

---

## 5. Les écarts

### 5.1 Gestion des clients Stripe — écart majeur

Le cahier des charges demande « création, mise à jour, association à une
entité tenant générique ». L'état réel :

- **Création** : uniquement en *lazy*, via `ensure_customer`. Aucune route
  HTTP.
- **Mise à jour** : `BillingProvider::update_customer` est déclarée sur le
  port, implémentée dans l'adapter Stripe… et **n'a aucun appelant**. Le
  double de test le dit explicitement (`crates/service/src/test_support.rs`) :
  *« `create_subscription` and `update_customer` have no caller among the use
  cases wired so far »*.

C'est du code mort sur une ligne explicite du cahier des charges. Une route
`PATCH /customer` plus le cas d'usage correspondant dans `service` ferment le
point pour une petite centaine de lignes.

### 5.2 Création d'abonnement : pas de chemin direct — écart moyen

`create_subscription` existe sur le port mais n'est appelée que par la
commande `seed`. Le seul chemin de création exposé par l'API est la Checkout
Session.

C'est **défendable** — Stripe recommande Checkout, et cela évite d'implémenter
soi-même la gestion du SCA — mais ce n'est pas ce que demande le texte. À
préparer comme une réponse argumentée, ou à combler par un `POST
/subscriptions`.

### 5.3 Facturation : la « génération » n'est pas couverte — écart mineur

« Statut de paiement » et « récupération » sont traités (liste paginée en
keyset avec curseur opaque, plus le détail unitaire). La « génération » n'a
pas d'équivalent : Stripe émet les factures et le module les mire par webhook.

C'est le bon design pour un miroir — mais il faut l'énoncer comme un choix
assumé, pas le laisser passer pour un oubli.

### 5.4 Bonus facultatifs absents — sans pénalité

Facturation à l'usage et session de portail client : les deux sont
explicitement listés hors scope dans le README. Le raisonnement sur le portail
est juste — il prendrait en charge le changement de plan et les moyens de
paiement, c'est-à-dire exactement le travail que ce module existe pour
montrer. Étant facultatifs, ils ne coûtent aucun point ; ils restent le levier
disponible pour pousser la note.

### 5.5 La rustdoc cite 62 fois des documents absents du dépôt — écart moyen

`.gitignore` exclut `docs/init-spec.md`, `docs/plan/`, `docs/spec/` et
`docs/intent/`. Or la documentation du code y renvoie **62 fois**
(`init-spec.md §7.4`, `docs/intent/phase-2.md`,
`docs/spec/phase-2-stripe-adapter.md`, …).

Concrètement : le lecteur clone le dépôt, lit « voir `init-spec.md §7.4` pour
l'argument complet », et n'a pas le fichier. Sur un livrable dont la
documentation est un critère annoncé, c'est le défaut le plus facilement
évitable de la liste. Deux issues : committer ces documents, ou inliner
l'argument dans la rustdoc.

### 5.6 Aucune intégration continue — écart moyen

Pas de `.github/workflows`. Tous les gates existent et passent — mais rien ne
les exécute. Pour un composant présenté comme branchable tel quel, un fichier
d'une trentaine de lignes (`fmt`, `clippy`, `test`, `typecheck`, `vitest`)
changerait la perception du sérieux opérationnel.

### 5.7 Détails d'exploitation — écart mineur

- Aucun timeout HTTP explicite sur le client Stripe : les défauts du SDK
  s'appliquent.
- `PgPoolOptions::new().connect(...)` sans réglage de pool.
- Pas de `/health`, pas d'arrêt gracieux — alors que la feature `signal` de
  tokio est activée dans `Cargo.toml`.
- Pas de description OpenAPI. Non demandée, mais attendue d'un composant
  réutilisable.

### 5.8 Frontend : correct, mais sous-exploite l'API — écart mineur

La structure est bonne : séparation `host/` / `billing/`, `host/` n'important
rien de `billing/`, états de chargement et d'erreur explicites, clés de cache
portant l'identifiant de tenant et construites par une fabrique unique. Le
traitement du retour de Checkout est particulièrement juste — la redirection
est traitée comme un signal d'UX et le webhook comme la vérité, les deux flux
sondant le miroir plutôt que d'afficher un succès optimiste.

Deux réserves :

- `useCancelSubscription` envoie systématiquement `at_period_end: false`.
  L'interface ne sait donc faire qu'une résiliation immédiate, alors que
  l'API expose les deux comportements.
- 8 tests front pour 6 composants, face à 305 tests côté Rust. Le déséquilibre
  se voit.

---

## 6. Recommandations, par rapport gain / effort

| # | Action | Ferme | Effort estimé |
|---|---|---|---|
| 1 | Committer `docs/`, ou inliner les références dans la rustdoc | §5.5 | ~30 min |
| 2 | `PATCH /customer` + le cas d'usage, supprimant le code mort | §5.1 | ~1 h |
| 3 | Un workflow GitHub Actions exécutant les gates existants | §5.6 | ~30 min |
| 4 | Exposer `at_period_end` dans l'interface | §5.8 | ~15 min |

Le point 1 est celui à traiter en premier : c'est le seul défaut visible avant
même d'avoir lu une ligne de logique.

---

## 7. Appréciation d'ensemble

Le dépôt est nettement au-dessus de la moyenne pour un exercice de
recrutement. Les parties difficiles — idempotence, vérification et traitement
des webhooks, isolation multi-tenant, frontières entre crates — sont traitées
avec un niveau de raisonnement qu'on rencontre rarement, et chaque décision
est argumentée à l'endroit du code qui l'implémente plutôt que dans un
document séparé.

Ce qui manque relève de **lignes explicites du cahier des charges**, pas de la
profondeur technique. Les quatre actions ci-dessus porteraient l'ensemble aux
environs de 95 sans toucher à l'architecture.
