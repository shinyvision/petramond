use serde::{Deserialize, Serialize};

use crate::player::PlayerId;
use petramond_math::math::IVec3;
use petramond_world::crafting::CraftingRecipeData;

use super::{ItemSlotWire, Transform};

/// One enabled mod, as the handshake reports it. Version is display-only —
/// compatibility checks are by ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModEntry {
    pub id: String,
    pub version: String,
}

/// What a joining client offers as its identity, beside the proof of its own
/// key.
///
/// Which one a client may send is not its choice: the server states its policy
/// in `HelloAck { requires_account }` and refuses the other kind. An online
/// server learns the session's name from the redeemed ticket, never from the
/// client — which is why there is no name field beside the ticket.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinCredential {
    /// A single-use Petramond join ticket, minted for this server's advertised
    /// `server_id`. The account's username becomes the session name.
    Ticket(String),
    /// A requested display name, for a server running with account checks
    /// off. The proven identity key is then the player.
    Name(String),
}

/// Why a `Join` was refused. A taken display name is NOT a refusal: it is
/// auto-deduped with a numeric suffix at admission (see
/// `ServerGame::begin_admission`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinRejectReason {
    /// The identity proof did not verify against this connection's challenge.
    BadProof,
    /// The requested display name is unacceptable; the text says why.
    InvalidName(String),
    /// This identity is already connected to the server.
    AlreadyConnected,
    /// Every player slot is in use.
    ServerFull,
    /// This server requires a Petramond account and the client offered a name.
    AccountRequired,
    /// This server checks no accounts, so it cannot redeem a ticket — and the
    /// ticket is the only place the account's name lives, so there is nothing to
    /// admit. The client must offer a name instead.
    AccountNotAccepted,
    /// This account is already playing on this server. An account is one
    /// session, never deduped with a suffix — checked by its stable id, so a
    /// rename cannot walk the same person in twice.
    AccountAlreadyOnline,
    /// The account service refused the client's ticket (expired, already used,
    /// minted for another server, or the account cannot play online).
    AccountRejected(String),
    /// The server could not reach the account service, so it cannot tell who
    /// this is. Nothing about the client is wrong; retrying may work.
    AccountUnavailable(String),
}

/// The server's registry name tables, in server-runtime-id order — the wire's
/// id vocabulary (the on-disk analogue is `palette.json`). The client builds
/// server-id→client-id LUTs from these at join.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameTables {
    pub blocks: Vec<String>,
    /// Surface biome registry keys, indexed by biome id (id 0 is unassigned:
    /// an empty key). Pack biomes register after the engine's in pack load
    /// order, so column biome bytes remap by key like every other id.
    pub biomes: Vec<String>,
    pub items: Vec<String>,
    pub mobs: Vec<String>,
    pub sounds: Vec<String>,
    pub effects: Vec<String>,
    /// `particle_emitters.json` bundle keys, in server-id order.
    pub emitters: Vec<String>,
    /// `conditions.json` keys, in server-id order.
    pub conditions: Vec<String>,
    /// Every registered player rig's animator vocabulary, in server rig-id
    /// order — the rig's name and the clips, params, slots and events its
    /// graph declares, in id order. The client matches rigs by NAME; a rig
    /// it lacks drops that rig's rows alone.
    pub animators: Vec<crate::player::animator::AnimatorNames>,
}

/// The joining player's own restored state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelfRestore {
    pub transform: Transform,
    /// `PlayerMode` as its discriminant (tiny closed enum).
    pub mode: u8,
    pub health: i32,
    pub bed_spawn: Option<(IVec3, IVec3)>,
    /// Active status effects by registry NAME + remaining ticks (names, not
    /// ids: effects are the one registry small enough that names are cheap and
    /// they already persist by name in level.dat).
    pub effects: Vec<(String, u32)>,
    /// All inventory slots in index order, then the cursor stack, then the
    /// off-hand stack last.
    pub inventory: Vec<Option<ItemSlotWire>>,
    /// The active hotbar slot, so the restored selection survives the join.
    pub active_slot: u8,
    /// The recipe browser's craftable-only filter preference.
    pub craft_craftable_only: bool,
    /// Every crafting recipe unlocked for this player, in unlock order — the
    /// browser shows exactly these. Later unlocks arrive as the appended
    /// suffix ([`ServerToClient::RecipesUnlocked`]).
    ///
    /// [`ServerToClient::RecipesUnlocked`]: crate::net::protocol::ServerToClient::RecipesUnlocked
    pub unlocked_recipes: Vec<String>,
}

/// Everything a client needs to enter the world, sent on `JoinAccept`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JoinData {
    pub player_id: PlayerId,
    /// The name this player was admitted under — the one every other client
    /// knows it by, which need not be the one it asked for.
    pub player_name: String,
    pub seed: u32,
    /// The day/night clock (`petramond:clock` contract) so the sky renders right
    /// from the first frame.
    pub clock: u64,
    pub tables: NameTables,
    pub self_restore: SelfRestore,
    /// The enabled player-crafting catalog, sent once and resolved locally by
    /// registry name. Unlike live slot contents, these rows carry no numeric
    /// registry ids and therefore need no transport remap.
    pub crafting_recipes: Vec<CraftingRecipeData>,
    /// Already-connected players (id, name), local player excluded.
    pub players: Vec<(PlayerId, String)>,
    /// What this server consents to about the joining client's own packs.
    pub client_policy: ClientPolicy,
}

/// What a server consents to about the client-side packs of the players who
/// join it — signalled, not enforced: a modified client can
/// ignore it; the honest client and every shipped pack obey it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientPolicy {
    /// Presentation-only packs (no simulation module, no world content) the
    /// server does not run may load on its players' clients.
    pub presentation_packs: bool,
}

impl Default for ClientPolicy {
    fn default() -> Self {
        Self {
            presentation_packs: true,
        }
    }
}
