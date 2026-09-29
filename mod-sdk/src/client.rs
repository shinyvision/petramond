use mod_api::{
    BlockId, ClientCanvasElement, ClientContext, ClientEngineFactsData, ClientKeyContexts,
    ClientKeyMods, ClientOverlayAnchor, ClientPackInfo, ClientStorageScope, ClientSurfaceColumn,
    ClientSurfaceQuery, ClientTextRun, ClientWallTime, ClientWorldMark, GuiValue,
};

#[allow(unused_imports)]
use crate::Mod;

use crate::__rt::{host_fn, try_host_fn};

host_fn! {
    pub fn client_register_overlay(
        image_key: &str,
        anchor: ClientOverlayAnchor,
        margin: [u16; 2],
        display_size: [u16; 2],
        hud: bool
    ) => ClientRegisterOverlay {
        image_key: image_key.into(),
        anchor,
        margin,
        display_size,
        hud,
    }
}

host_fn! {
    pub fn client_register_key(
        id: &str,
        label: &str,
        key: &str,
        mods: ClientKeyMods,
        contexts: ClientKeyContexts,
        action_id: u32
    ) => ClientRegisterKey {
        id: id.into(),
        label: label.into(),
        key: key.into(),
        mods,
        contexts,
        action_id,
    }
}

host_fn! {
    pub fn client_key_labels(ids: Vec<String>) -> Vec<Option<String>>
        => ClientKeyLabels { ids } => Names
}

host_fn! {
    pub fn client_surface_columns(queries: Vec<ClientSurfaceQuery>) -> Vec<Option<ClientSurfaceColumn>>
        => ClientSurfaceColumns { queries } => ClientSurfaceColumns
}

host_fn! {
    pub fn client_blocks_at(positions: Vec<[i32; 3]>) -> Vec<Option<BlockId>>
        => ClientBlocksAt { positions } => Blocks
}

host_fn! {
    pub fn client_cell_kv_at(key: &str, cells: Vec<[i32; 3]>) -> Vec<Option<Vec<u8>>>
        => ClientCellKvAt { key: key.into(), cells } => BytesMany
}

host_fn! {
    pub fn client_image_blit(key: &str, origin: [u16; 2], size: [u16; 2], rgba: Vec<u8>)
        => ClientImageBlit { key: key.into(), origin, size, rgba }
}

host_fn! {
    pub fn client_ui_state_set(key: &str, value: GuiValue)
        => ClientUiStateSet { key: key.into(), value }
}

host_fn! {
    pub fn client_ui_state_get(key: &str) -> Option<GuiValue>
        => ClientUiStateGet { key: key.into() } => GuiValue
}

host_fn! {
    pub fn client_image_set(key: &str, width: u16, height: u16, rgba: Vec<u8>)
        => ClientImageSet { key: key.into(), width, height, rgba }
}

host_fn! {
    pub fn client_text_measure(text: &str, scale: u8) -> [u16; 2]
        => ClientTextMeasure { text: text.into(), scale } => ClientTextSize
}

host_fn! {
    pub fn client_image_draw_texts(key: &str, runs: Vec<ClientTextRun>)
        => ClientImageDrawTexts { key: key.into(), runs }
}

host_fn! {
    pub fn client_gui_open(kind_key: &str) -> bool
        => ClientGuiOpen { kind_key: kind_key.into() } => Bool
}

host_fn! {
    pub fn client_gui_close() => ClientGuiClose
}

host_fn! {
    pub fn client_ui_focus(id: &str, item: Option<u32>) -> bool
        => ClientUiFocus { id: id.into(), item } => Bool
}

host_fn! {
    pub fn client_pause_open() -> bool => ClientPauseOpen => Bool
}

host_fn! {
    pub fn client_canvas_open(canvas_key: &str, size: [u16; 2]) -> bool
        => ClientCanvasOpen { canvas_key: canvas_key.into(), size } => Bool
}

host_fn! {
    pub fn client_canvas_close() => ClientCanvasClose
}

host_fn! {
    pub fn client_canvas_scene_set(canvas_key: &str, elements: Vec<ClientCanvasElement>)
        => ClientCanvasSceneSet { canvas_key: canvas_key.into(), elements }
}

host_fn! {
    pub fn client_canvas_view_set(canvas_key: &str, offset: [f32; 2])
        => ClientCanvasViewSet { canvas_key: canvas_key.into(), offset }
}

pub fn client_storage_get_many(
    scope: ClientStorageScope,
    keys: Vec<String>,
) -> Vec<Option<Vec<u8>>> {
    crate::__rt::expect_value(
        "ClientStorageGetMany",
        crate::__rt::call(&mod_api::calls::ClientStorageGetMany { scope, keys })
            .decode_as(mod_api::ret_decode::ClientStorageValues),
    )
    .into_iter()
    .map(|value| value.map(mod_api::ByteBuf::into_vec))
    .collect()
}

try_host_fn! {
    pub fn client_storage_set_many(
        scope: ClientStorageScope,
        entries: Vec<(String, Option<Vec<u8>>)>
    ) -> u64
        => ClientStorageSetMany {
            scope,
            entries: entries
                .into_iter()
                .map(|(key, value)| (key, value.map(mod_api::ByteBuf::from)))
                .collect(),
        } => ClientStorageWrite
}

pub fn client_storage_write_poll(
    scope: ClientStorageScope,
    ticket: u64,
) -> Option<Result<(), mod_api::HostError>> {
    let landed = crate::__rt::recoverable_value(
        "ClientStorageWritePoll",
        crate::__rt::call(&mod_api::calls::ClientStorageWritePoll { scope, ticket })
            .decode_as(mod_api::ret_decode::ClientStorageWritten),
    );
    match landed {
        Ok(landed) => landed.then_some(Ok(())),
        Err(refused) => Some(Err(refused)),
    }
}

host_fn! {
    pub fn client_storage_read_begin(scope: ClientStorageScope, keys: Vec<String>) -> u64
        => ClientStorageReadBegin { scope, keys } => U64
}

pub fn client_storage_read_poll(
    scope: ClientStorageScope,
    ticket: u64,
) -> Option<Vec<Option<Vec<u8>>>> {
    crate::__rt::expect_value(
        "ClientStorageReadPoll",
        crate::__rt::call(&mod_api::calls::ClientStorageReadPoll { scope, ticket })
            .decode_as(mod_api::ret_decode::ClientStorageRead),
    )
    .map(|values| {
        values
            .into_iter()
            .map(|value| value.map(mod_api::ByteBuf::into_vec))
            .collect()
    })
}

host_fn! {
    pub fn client_env_params(keys: &[&str]) -> Vec<Option<[f32; 4]>>
        => ClientEnvParams { keys: keys.iter().map(|k| (*k).into()).collect() }
        => EnvParams
}

host_fn! {
    pub fn client_biome_at(pos: [i32; 2]) -> Option<u8> => ClientBiomeAt { pos } => MaybeByte
}

host_fn! {
    pub fn client_ambient_set(key: &str, intensity: f32, wind: [f32; 2]) -> bool
        => ClientAmbientSet { key: key.into(), intensity, wind } => Bool
}

host_fn! {
    pub fn client_loop_set(key: &str, gain: f32) -> bool
        => ClientLoopSet { key: key.into(), gain } => Bool
}

host_fn! {
    pub fn client_mood_set(darken: f32, desaturate: f32) -> bool
        => ClientMoodSet { darken, desaturate } => Bool
}

host_fn! {
    pub fn client_world_marks_set(set: &str, marks: Vec<ClientWorldMark>)
        => ClientWorldMarksSet { set: set.into(), marks }
}

host_fn! {
    pub fn client_context() -> ClientContext => ClientContext => ClientContext
}

host_fn! {
    pub fn client_engine_facts() -> ClientEngineFactsData
        => ClientEngineFacts => ClientEngineFacts
}

host_fn! {
    pub fn client_packs() -> Vec<ClientPackInfo> => ClientPacks => ClientPacks
}

host_fn! {
    pub fn client_wall_clock() -> ClientWallTime => ClientWallClock => ClientWallClock
}
