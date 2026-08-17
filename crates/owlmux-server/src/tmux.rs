use std::{
    collections::{HashSet, VecDeque},
    time::Duration,
};

use serde::Serialize;
use tokio::time::timeout;
use uuid::Uuid;

use crate::{
    generated::contracts::{
        ATTACHMENT_MAX_PANE_SNAPSHOT_BYTES, ATTACHMENT_MAX_PANES, ATTACHMENT_MAX_PROJECTION_BYTES,
        ATTACHMENT_MAX_TERMINAL_CHUNK_BYTES,
    },
    ssh::{ControlChild, ProbeOutput, SshError},
};

const PROBE_CLIENT_PREFIX: &str = "__OWLMUX_TMUX_CLIENT_V1__\t";
const PROBE_SERVER_PREFIX: &str = "__OWLMUX_TMUX_SERVER_V1__\t";
const MAX_SESSIONS: usize = 128;
const MAX_RESPONSE_BYTES: usize = ATTACHMENT_MAX_PROJECTION_BYTES;
const MAX_PANE_METADATA_BYTES: usize = 1024;
const MAX_QUEUED_EVENTS: usize = 256;
const MAX_QUEUED_OUTPUT_BYTES: usize = 512 * 1024;
const MAX_TEXT_BYTES: usize = 256;
const MAX_LAYOUT_BYTES: usize = 4096;
const MAX_DIMENSION: u32 = 10_000;
const MAX_HYDRATION_ATTEMPTS: usize = 3;
const MAX_RESPONSE_RECORDS: usize = 16_384;
const MAX_RESPONSE_WIRE_BYTES: usize = 8 * 1024 * 1024;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
const PANE_FORMAT: &str = "#{pane_id}:#{window_id}:#{pane_active}:#{pane_width}:#{pane_height}:#{pane_left}:#{pane_top}:#{cursor_x}:#{cursor_y}:#{alternate_on}:#{cursor_flag}:#{insert_flag}:#{keypad_cursor_flag}:#{keypad_flag}:#{origin_flag}:#{wrap_flag}:#{scroll_region_upper}:#{scroll_region_lower}";
const KNOWN_BAD_VERSIONS: &[&str] = &[];

#[derive(Clone, Debug, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub session_created: i64,
    pub name: String,
    pub attached_client_count: u32,
    pub window_count: u32,
}

pub struct ProbeResult {
    pub tmux_client_version: String,
    pub tmux_server_version: Option<String>,
    pub selection_epoch: Uuid,
    pub sessions: Vec<SessionSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WindowProjection {
    pub window_id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub layout: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct PaneProjection {
    pub pane_id: String,
    pub active: bool,
    pub width: u32,
    pub height: u32,
    pub left: u32,
    pub top: u32,
    pub title: String,
    pub current_command: String,
    #[serde(skip)]
    terminal: PaneTerminalState,
}

#[derive(Clone, Debug)]
struct PaneTerminalState {
    cursor_x: u32,
    cursor_y: u32,
    modes: u8,
    scroll_upper: u32,
    scroll_lower: u32,
}

const MODE_ALTERNATE: u8 = 1 << 0;
const MODE_CURSOR_VISIBLE: u8 = 1 << 1;
const MODE_INSERT: u8 = 1 << 2;
const MODE_APPLICATION_CURSOR: u8 = 1 << 3;
const MODE_APPLICATION_KEYPAD: u8 = 1 << 4;
const MODE_ORIGIN: u8 = 1 << 5;
const MODE_WRAP: u8 = 1 << 6;

impl PaneTerminalState {
    const fn has_mode(&self, mode: u8) -> bool {
        self.modes & mode != 0
    }
}

pub struct PaneSnapshot {
    pub pane_id: String,
    pub content: Vec<u8>,
}

pub struct WorkspaceProjection {
    pub window: WindowProjection,
    pub panes: Vec<PaneProjection>,
    pub snapshots: Vec<PaneSnapshot>,
}

#[derive(Debug)]
pub enum ControlEvent {
    Output { pane_id: String, data: Vec<u8> },
    Refresh,
    Exit,
}

pub struct ControlAdapter {
    control: ControlChild,
    events: VecDeque<ControlEvent>,
    queued_output_bytes: usize,
}

impl ControlAdapter {
    /// Qualify an attached control client and enable bounded pause-after flow control.
    ///
    /// # Errors
    ///
    /// Returns a transport, target-command, or control-framing error.
    pub async fn start(control: ControlChild) -> Result<Self, TmuxError> {
        let mut adapter = Self {
            control,
            events: VecDeque::new(),
            queued_output_bytes: 0,
        };
        adapter.read_response().await?;
        adapter
            .command(
                "refresh-client -f read-only,ignore-size,pause-after=1",
                MAX_RESPONSE_BYTES,
            )
            .await?;
        adapter
            .command(
                "refresh-client -B 'owlmux-pane-v1:%*:#{pane_title}|#{pane_current_command}|#{pane_active}|#{pane_width}|#{pane_height}|#{pane_left}|#{pane_top}'",
                MAX_RESPONSE_BYTES,
            )
            .await?;
        adapter.clear_events();
        Ok(adapter)
    }

    /// Build one bounded target-current projection while pane delivery to this client is paused.
    ///
    /// # Errors
    ///
    /// Returns a target-change, cardinality, transport, or framing error.
    pub async fn hydrate(
        &mut self,
        session_id: &str,
        session_created: i64,
    ) -> Result<WorkspaceProjection, TmuxError> {
        for _ in 0..MAX_HYDRATION_ATTEMPTS {
            self.clear_events();
            match self.hydrate_once(session_id, session_created).await {
                Ok(projection) => return Ok(projection),
                Err(TmuxError::Changed) => {}
                Err(error) => return Err(error),
            }
        }
        Err(TmuxError::Changed)
    }

    /// Read and queue one fresh control notification while another bounded sink operation runs.
    ///
    /// # Errors
    ///
    /// Returns a transport or control-framing error.
    pub async fn pump_event(&mut self) -> Result<(), TmuxError> {
        let line = self.control.next_line().await?;
        self.ingest_notification(&line)
    }

    /// Return one queued or newly parsed control event.
    ///
    /// # Errors
    ///
    /// Returns a transport or control-framing error.
    pub async fn next_event(&mut self) -> Result<ControlEvent, TmuxError> {
        loop {
            if let Some(event) = self.pop_event() {
                return Ok(event);
            }
            let line = self.control.next_line().await?;
            self.ingest_notification(&line)?;
        }
    }

    async fn hydrate_once(
        &mut self,
        session_id: &str,
        session_created: i64,
    ) -> Result<WorkspaceProjection, TmuxError> {
        let window = self.observe_window(session_id, session_created).await?;
        let mut panes = self.observe_panes(&window).await?;
        self.observe_pane_labels(&mut panes).await?;
        self.pause_panes(&panes).await?;
        self.require_stable_cutover()?;
        self.continue_panes(&panes).await?;
        let snapshots = self.capture_panes(&window, &mut panes).await?;
        self.revalidate_projection(session_id, session_created, &window, &panes)
            .await?;
        self.require_live_cutover()?;
        Ok(WorkspaceProjection {
            window,
            panes,
            snapshots,
        })
    }

    async fn observe_window(
        &mut self,
        session_id: &str,
        session_created: i64,
    ) -> Result<WindowProjection, TmuxError> {
        if !is_session_id(session_id) || session_created <= 0 {
            return Err(TmuxError::Protocol);
        }
        let response = self
            .command(
                &format!(
                    "display-message -p -t {session_id} '#{{session_id}}:#{{session_created}}:#{{window_id}}:#{{window_width}}:#{{window_height}}:#{{window_layout}}'"
                ),
                MAX_RESPONSE_BYTES,
            )
            .await?;
        let mut fields = single_line(&response)?.splitn(6, ':');
        let observed_session = fields.next().ok_or(TmuxError::Protocol)?;
        let observed_created = parse_positive_i64(fields.next().ok_or(TmuxError::Protocol)?)?;
        let window_id = fields.next().ok_or(TmuxError::Protocol)?;
        let width = parse_dimension(fields.next().ok_or(TmuxError::Protocol)?)?;
        let height = parse_dimension(fields.next().ok_or(TmuxError::Protocol)?)?;
        let layout = fields.next().ok_or(TmuxError::Protocol)?;
        if observed_session != session_id
            || observed_created != session_created
            || !is_window_id(window_id)
            || layout.is_empty()
            || layout.len() > MAX_LAYOUT_BYTES
            || !layout.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(TmuxError::Changed);
        }
        let name = self
            .command(
                &format!("display-message -p -t {window_id} '#{{window_name}}'"),
                MAX_RESPONSE_BYTES,
            )
            .await?;
        Ok(WindowProjection {
            window_id: window_id.to_owned(),
            name: safe_text(&name, 128)?,
            width,
            height,
            layout: layout.to_owned(),
        })
    }

    async fn observe_panes(
        &mut self,
        window: &WindowProjection,
    ) -> Result<Vec<PaneProjection>, TmuxError> {
        let rows = self
            .command(
                &format!("list-panes -t {} -F '{PANE_FORMAT}'", window.window_id),
                MAX_RESPONSE_BYTES,
            )
            .await?;
        parse_pane_rows(&rows, window)
    }

    async fn observe_pane_labels(&mut self, panes: &mut [PaneProjection]) -> Result<(), TmuxError> {
        for pane in panes {
            let title = self
                .command(
                    &format!("display-message -p -t {} '#{{pane_title}}'", pane.pane_id),
                    MAX_RESPONSE_BYTES,
                )
                .await?;
            pane.title = safe_text(&title, MAX_TEXT_BYTES)?;
            let current_command = self
                .command(
                    &format!(
                        "display-message -p -t {} '#{{pane_current_command}}'",
                        pane.pane_id
                    ),
                    MAX_RESPONSE_BYTES,
                )
                .await?;
            pane.current_command = safe_text(&current_command, MAX_TEXT_BYTES)?;
        }
        Ok(())
    }

    async fn pause_panes(&mut self, panes: &[PaneProjection]) -> Result<(), TmuxError> {
        for pane in panes {
            self.command(
                &format!("refresh-client -A '{}:pause'", pane.pane_id),
                MAX_RESPONSE_BYTES,
            )
            .await?;
        }
        Ok(())
    }

    async fn continue_panes(&mut self, panes: &[PaneProjection]) -> Result<(), TmuxError> {
        for pane in panes {
            self.command(
                &format!("refresh-client -A '{}:continue'", pane.pane_id),
                MAX_RESPONSE_BYTES,
            )
            .await?;
        }
        Ok(())
    }

    async fn capture_panes(
        &mut self,
        window: &WindowProjection,
        panes: &mut [PaneProjection],
    ) -> Result<Vec<PaneSnapshot>, TmuxError> {
        let mut snapshots = Vec::with_capacity(panes.len());
        let mut projection_bytes = 0_usize;
        for pane in panes.iter_mut() {
            let pane_id = pane.pane_id.clone();
            let (metadata, lines) = self.capture_barrier(&pane_id).await?;
            let final_pane = parse_pane_row(&metadata, window)?;
            if !same_pane_structure(pane, &final_pane) {
                return Err(TmuxError::Changed);
            }
            pane.terminal = final_pane.terminal;
            let content = capture_lines(&lines, pane)?;
            projection_bytes = projection_bytes
                .checked_add(content.len())
                .ok_or(TmuxError::Protocol)?;
            if projection_bytes > ATTACHMENT_MAX_PROJECTION_BYTES {
                return Err(TmuxError::ProjectionTooLarge);
            }
            self.discard_captured_output(&pane_id);
            snapshots.push(PaneSnapshot { pane_id, content });
        }
        Ok(snapshots)
    }

    async fn capture_barrier(
        &mut self,
        pane_id: &str,
    ) -> Result<(Vec<u8>, Vec<Vec<u8>>), TmuxError> {
        let command = format!(
            "capture-pane -p -e -N -t {pane_id} ; display-message -p -t {pane_id} '{PANE_FORMAT}'"
        );
        if let Err(error) = self.control.send_command(&command).await {
            tracing::warn!(?error, "tmux capture barrier write failed");
            return Err(error.into());
        }
        // Qualified releases emit one guard per command in the input command list. Capturing first
        // preserves the qualified capture-to-live ordering. The event boundary rejects a final
        // metadata observation separated from that capture by pane output.
        response_deadline(RESPONSE_TIMEOUT, async {
            let lines = self
                .read_response_with_limit_inner(ATTACHMENT_MAX_PANE_SNAPSHOT_BYTES)
                .await?;
            let event_boundary = self.events.len();
            let metadata = self
                .read_response_with_limit_inner(MAX_PANE_METADATA_BYTES)
                .await?;
            if self.events.iter().skip(event_boundary).any(
                |event| matches!(event, ControlEvent::Output { pane_id: output_pane, .. } if output_pane == pane_id),
            ) {
                return Err(TmuxError::Changed);
            }
            let metadata = single_line(&metadata)?.as_bytes().to_vec();
            Ok((metadata, lines))
        })
        .await
        .inspect_err(|error| {
            tracing::warn!(?error, "tmux capture barrier response failed");
        })
    }

    async fn revalidate_projection(
        &mut self,
        session_id: &str,
        session_created: i64,
        expected_window: &WindowProjection,
        expected_panes: &[PaneProjection],
    ) -> Result<(), TmuxError> {
        let window = self.observe_window(session_id, session_created).await?;
        if window != *expected_window {
            return Err(TmuxError::Changed);
        }
        let panes = self.observe_panes(&window).await?;
        if panes.len() != expected_panes.len()
            || panes
                .iter()
                .zip(expected_panes)
                .any(|(observed, expected)| !same_pane_structure(observed, expected))
        {
            return Err(TmuxError::Changed);
        }
        Ok(())
    }

    fn require_stable_cutover(&mut self) -> Result<(), TmuxError> {
        let mut changed = false;
        while let Some(event) = self.pop_event() {
            match event {
                ControlEvent::Output { .. } => {}
                ControlEvent::Refresh => changed = true,
                ControlEvent::Exit => return Err(TmuxError::Target),
            }
        }
        if changed {
            Err(TmuxError::Changed)
        } else {
            Ok(())
        }
    }

    fn discard_captured_output(&mut self, pane_id: &str) {
        self.queued_output_bytes = discard_pre_capture_output(&mut self.events, pane_id);
    }

    fn require_live_cutover(&self) -> Result<(), TmuxError> {
        if self
            .events
            .iter()
            .any(|event| matches!(event, ControlEvent::Exit))
        {
            return Err(TmuxError::Target);
        }
        if self
            .events
            .iter()
            .any(|event| matches!(event, ControlEvent::Refresh))
        {
            return Err(TmuxError::Changed);
        }
        Ok(())
    }

    async fn command(
        &mut self,
        command: &str,
        max_response_bytes: usize,
    ) -> Result<Vec<Vec<u8>>, TmuxError> {
        let operation = command.split_ascii_whitespace().next().unwrap_or("unknown");
        if let Err(error) = self.control.send_command(command).await {
            tracing::warn!(operation, ?error, "tmux control command write failed");
            return Err(error.into());
        }
        self.read_response_with_limit(max_response_bytes)
            .await
            .inspect_err(|error| {
                tracing::warn!(operation, ?error, "tmux control command response failed");
            })
    }

    async fn read_response(&mut self) -> Result<Vec<Vec<u8>>, TmuxError> {
        self.read_response_with_limit(MAX_RESPONSE_BYTES).await
    }

    async fn read_response_with_limit(
        &mut self,
        max_response_bytes: usize,
    ) -> Result<Vec<Vec<u8>>, TmuxError> {
        response_deadline(
            RESPONSE_TIMEOUT,
            self.read_response_with_limit_inner(max_response_bytes),
        )
        .await
    }

    async fn read_response_with_limit_inner(
        &mut self,
        max_response_bytes: usize,
    ) -> Result<Vec<Vec<u8>>, TmuxError> {
        let mut marker = None;
        let mut size = 0_usize;
        let mut budget = ResponseBudget::default();
        let mut result = Vec::new();
        loop {
            let line = self.control.next_line().await?;
            budget.observe(&line)?;
            if marker.is_none() {
                if let Some(begin) = parse_marker(&line, b"%begin ")? {
                    marker = Some(begin);
                } else {
                    self.ingest_notification(&line)?;
                }
                continue;
            }
            // Inside a guarded response every non-marker record is opaque command payload: a
            // captured terminal row may legitimately begin with `%`. Qualified tmux releases keep
            // unrelated notifications ordered outside the matching guard. The explicit
            // pause/continue commands may guard their own same-state notification; their callers
            // intentionally ignore command payload and revalidate projection state afterward.
            if let Some(end) = parse_marker(&line, b"%end ")? {
                return if marker == Some(end) {
                    Ok(result)
                } else {
                    Err(TmuxError::Protocol)
                };
            }
            if let Some(error) = parse_marker(&line, b"%error ")? {
                return if marker == Some(error) {
                    Err(TmuxError::Target)
                } else {
                    Err(TmuxError::Protocol)
                };
            }
            if line.starts_with(b"%begin ") {
                return Err(TmuxError::Protocol);
            }
            let payload = command_payload(&line);
            size = size.checked_add(payload.len()).ok_or(TmuxError::Protocol)?;
            if size > max_response_bytes {
                return Err(TmuxError::ProjectionTooLarge);
            }
            result.push(payload.to_vec());
        }
    }

    fn ingest_notification(&mut self, line: &[u8]) -> Result<(), TmuxError> {
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        if let Some(rest) = line.strip_prefix(b"%output ") {
            let (pane_id, data) = split_once_space(rest)?;
            self.queue_output(pane_id, data)?;
            return Ok(());
        }
        if let Some(rest) = line.strip_prefix(b"%extended-output ") {
            let first = rest
                .iter()
                .position(|byte| *byte == b' ')
                .ok_or(TmuxError::Protocol)?;
            let pane_id = &rest[..first];
            let remaining = &rest[first + 1..];
            let separator = remaining
                .windows(3)
                .position(|window| window == b" : ")
                .ok_or(TmuxError::Protocol)?;
            let age = &remaining[..separator];
            if age.is_empty() || !age.iter().all(u8::is_ascii_digit) {
                return Err(TmuxError::Protocol);
            }
            self.queue_output(pane_id, &remaining[separator + 3..])?;
            return Ok(());
        }
        if line == b"%exit" || line.starts_with(b"%exit ") {
            self.queue_event(ControlEvent::Exit)?;
            return Ok(());
        }
        if let Some(pane) = line.strip_prefix(b"%pause ") {
            let pane = std::str::from_utf8(pane).map_err(|_| TmuxError::Protocol)?;
            if !is_pane_id(pane) {
                return Err(TmuxError::Protocol);
            }
            self.queue_event(ControlEvent::Refresh)?;
            return Ok(());
        }
        if let Some(pane) = line.strip_prefix(b"%continue ") {
            let pane = std::str::from_utf8(pane).map_err(|_| TmuxError::Protocol)?;
            return if is_pane_id(pane) {
                Ok(())
            } else {
                Err(TmuxError::Protocol)
            };
        }
        if is_refresh_notification(line) {
            self.queue_event(ControlEvent::Refresh)?;
            return Ok(());
        }
        if is_ignorable_notification(line) {
            return Ok(());
        }
        Err(TmuxError::Protocol)
    }

    fn queue_output(&mut self, pane_id: &[u8], encoded: &[u8]) -> Result<(), TmuxError> {
        let pane_id = std::str::from_utf8(pane_id).map_err(|_| TmuxError::Protocol)?;
        if !is_pane_id(pane_id) {
            return Err(TmuxError::Protocol);
        }
        let decoded = decode_escaped(encoded)?;
        if decoded.is_empty() {
            return Ok(());
        }
        for chunk in decoded.chunks(ATTACHMENT_MAX_TERMINAL_CHUNK_BYTES) {
            self.queued_output_bytes = self
                .queued_output_bytes
                .checked_add(chunk.len())
                .ok_or(TmuxError::Protocol)?;
            if self.queued_output_bytes > MAX_QUEUED_OUTPUT_BYTES {
                return Err(TmuxError::Protocol);
            }
            self.queue_event(ControlEvent::Output {
                pane_id: pane_id.to_owned(),
                data: chunk.to_vec(),
            })?;
        }
        Ok(())
    }

    fn queue_event(&mut self, event: ControlEvent) -> Result<(), TmuxError> {
        if self.events.len() >= MAX_QUEUED_EVENTS {
            return Err(TmuxError::Protocol);
        }
        self.events.push_back(event);
        Ok(())
    }

    fn pop_event(&mut self) -> Option<ControlEvent> {
        let event = self.events.pop_front()?;
        if let ControlEvent::Output { data, .. } = &event {
            self.queued_output_bytes = self.queued_output_bytes.saturating_sub(data.len());
        }
        Some(event)
    }

    fn clear_events(&mut self) {
        self.events.clear();
        self.queued_output_bytes = 0;
    }
}

fn discard_pre_capture_output(events: &mut VecDeque<ControlEvent>, pane_id: &str) -> usize {
    events.retain(|event| {
        !matches!(event, ControlEvent::Output { pane_id: output_pane, .. } if output_pane == pane_id)
    });
    events
        .iter()
        .filter_map(|event| match event {
            ControlEvent::Output { data, .. } => Some(data.len()),
            ControlEvent::Refresh | ControlEvent::Exit => None,
        })
        .sum()
}

#[derive(Default)]
struct ResponseBudget {
    records: usize,
    wire_bytes: usize,
}

impl ResponseBudget {
    fn observe(&mut self, line: &[u8]) -> Result<(), TmuxError> {
        self.records = self.records.checked_add(1).ok_or(TmuxError::Protocol)?;
        self.wire_bytes = self
            .wire_bytes
            .checked_add(line.len())
            .ok_or(TmuxError::Protocol)?;
        if self.records > MAX_RESPONSE_RECORDS || self.wire_bytes > MAX_RESPONSE_WIRE_BYTES {
            return Err(TmuxError::Protocol);
        }
        Ok(())
    }
}

async fn response_deadline<F, T>(deadline: Duration, future: F) -> Result<T, TmuxError>
where
    F: std::future::Future<Output = Result<T, TmuxError>>,
{
    timeout(deadline, future)
        .await
        .map_err(|_| TmuxError::Deadline)?
}

fn parse_pane_rows(
    rows: &[Vec<u8>],
    window: &WindowProjection,
) -> Result<Vec<PaneProjection>, TmuxError> {
    if rows.is_empty() || rows.len() > ATTACHMENT_MAX_PANES {
        return Err(TmuxError::TooManyPanes);
    }
    let mut panes = Vec::with_capacity(rows.len());
    let mut pane_ids = HashSet::new();
    for row in rows {
        let pane = parse_pane_row(row, window)?;
        if !pane_ids.insert(pane.pane_id.clone()) {
            return Err(TmuxError::Protocol);
        }
        panes.push(pane);
    }
    if panes.iter().filter(|pane| pane.active).count() != 1 {
        return Err(TmuxError::Protocol);
    }
    Ok(panes)
}

fn parse_pane_row(row: &[u8], window: &WindowProjection) -> Result<PaneProjection, TmuxError> {
    let row = std::str::from_utf8(row).map_err(|_| TmuxError::Protocol)?;
    let fields = row.split(':').collect::<Vec<_>>();
    if fields.len() != 18 {
        return Err(TmuxError::Protocol);
    }
    let pane_id = fields[0];
    if !is_pane_id(pane_id) || fields[1] != window.window_id {
        return Err(TmuxError::Protocol);
    }
    let active = match fields[2] {
        "0" => false,
        "1" => true,
        _ => return Err(TmuxError::Protocol),
    };
    let width = parse_dimension(fields[3])?;
    let height = parse_dimension(fields[4])?;
    let left = parse_coordinate(fields[5])?;
    let top = parse_coordinate(fields[6])?;
    if left >= window.width
        || top >= window.height
        || left.saturating_add(width) > window.width
        || top.saturating_add(height) > window.height
    {
        return Err(TmuxError::Protocol);
    }
    let terminal = PaneTerminalState {
        cursor_x: parse_coordinate_with_bound(fields[7], width)?,
        cursor_y: parse_coordinate_with_bound(fields[8], height)?,
        modes: parse_modes(&fields[9..16])?,
        scroll_upper: parse_coordinate_with_bound(fields[16], height)?,
        scroll_lower: parse_coordinate_with_bound(fields[17], height)?,
    };
    if terminal.scroll_upper > terminal.scroll_lower
        || terminal.cursor_y < terminal.scroll_upper && terminal.has_mode(MODE_ORIGIN)
    {
        return Err(TmuxError::Protocol);
    }
    Ok(PaneProjection {
        pane_id: pane_id.to_owned(),
        active,
        width,
        height,
        left,
        top,
        title: String::new(),
        current_command: String::new(),
        terminal,
    })
}

fn same_pane_structure(left: &PaneProjection, right: &PaneProjection) -> bool {
    left.pane_id == right.pane_id
        && left.active == right.active
        && left.width == right.width
        && left.height == right.height
        && left.left == right.left
        && left.top == right.top
}

fn capture_lines(lines: &[Vec<u8>], pane: &PaneProjection) -> Result<Vec<u8>, TmuxError> {
    let terminal = &pane.terminal;
    let mut content = Vec::new();
    content.extend_from_slice(b"\x1bc");
    if terminal.has_mode(MODE_ALTERNATE) {
        content.extend_from_slice(b"\x1b[?1049h");
    }
    content.extend_from_slice(b"\x1b[?25l\x1b[?6l\x1b[H\x1b[2J");
    for (index, line) in lines.iter().enumerate() {
        if index != 0 {
            content.extend_from_slice(b"\r\n");
        }
        content.extend_from_slice(line);
        if content.len() > ATTACHMENT_MAX_PANE_SNAPSHOT_BYTES {
            return Err(TmuxError::ProjectionTooLarge);
        }
    }
    let scroll_upper = terminal.scroll_upper + 1;
    let scroll_lower = terminal.scroll_lower + 1;
    content.extend_from_slice(format!("\x1b[{scroll_upper};{scroll_lower}r").as_bytes());
    content.extend_from_slice(if terminal.has_mode(MODE_ORIGIN) {
        b"\x1b[?6h".as_slice()
    } else {
        b"\x1b[?6l".as_slice()
    });
    let cursor_row = if terminal.has_mode(MODE_ORIGIN) {
        terminal.cursor_y - terminal.scroll_upper + 1
    } else {
        terminal.cursor_y + 1
    };
    let cursor_column = terminal.cursor_x + 1;
    content.extend_from_slice(format!("\x1b[{cursor_row};{cursor_column}H").as_bytes());
    append_mode(
        &mut content,
        terminal.has_mode(MODE_INSERT),
        b"\x1b[4h",
        b"\x1b[4l",
    );
    append_mode(
        &mut content,
        terminal.has_mode(MODE_WRAP),
        b"\x1b[?7h",
        b"\x1b[?7l",
    );
    append_mode(
        &mut content,
        terminal.has_mode(MODE_APPLICATION_CURSOR),
        b"\x1b[?1h",
        b"\x1b[?1l",
    );
    append_mode(
        &mut content,
        terminal.has_mode(MODE_APPLICATION_KEYPAD),
        b"\x1b=",
        b"\x1b>",
    );
    append_mode(
        &mut content,
        terminal.has_mode(MODE_CURSOR_VISIBLE),
        b"\x1b[?25h",
        b"\x1b[?25l",
    );
    if content.len() > ATTACHMENT_MAX_PANE_SNAPSHOT_BYTES {
        return Err(TmuxError::ProjectionTooLarge);
    }
    Ok(content)
}

fn append_mode(output: &mut Vec<u8>, enabled: bool, set: &[u8], reset: &[u8]) {
    output.extend_from_slice(if enabled { set } else { reset });
}

/// Parse one bounded fixed probe result into a fresh selection epoch.
///
/// # Errors
///
/// Returns an incompatibility, cardinality, or framing error.
pub fn parse_probe(output: &ProbeOutput) -> Result<ProbeResult, TmuxError> {
    let mut lines = output.stdout.lines();
    let first = lines.next().ok_or(TmuxError::Protocol)?;
    let client_version = first
        .strip_prefix(PROBE_CLIENT_PREFIX)
        .ok_or(TmuxError::Protocol)?;
    validate_version(client_version)?;
    let second = lines.next().ok_or(TmuxError::Protocol)?;
    let server_value = second
        .strip_prefix(PROBE_SERVER_PREFIX)
        .ok_or(TmuxError::Protocol)?;
    let server_version = if server_value == "none" {
        None
    } else {
        validate_version(server_value)?;
        Some(server_value.to_owned())
    };

    let mut sessions = Vec::new();
    let mut identities = HashSet::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if sessions.len() >= MAX_SESSIONS {
            return Err(TmuxError::TooManySessions);
        }
        let mut fields = line.splitn(5, ':');
        let session_id = fields.next().ok_or(TmuxError::Protocol)?;
        let created = fields.next().ok_or(TmuxError::Protocol)?;
        let attached = fields.next().ok_or(TmuxError::Protocol)?;
        let windows = fields.next().ok_or(TmuxError::Protocol)?;
        let name = fields.next().ok_or(TmuxError::Protocol)?;
        if !is_session_id(session_id) || name.len() > 128 || name.chars().any(char::is_control) {
            return Err(TmuxError::Protocol);
        }
        let session_created = parse_positive_i64(created)?;
        let attached_client_count = parse_count(attached)?;
        let window_count = parse_count(windows)?;
        if window_count == 0 || !identities.insert((session_id.to_owned(), session_created)) {
            return Err(TmuxError::Protocol);
        }
        sessions.push(SessionSummary {
            session_id: session_id.to_owned(),
            session_created,
            name: name.to_owned(),
            attached_client_count,
            window_count,
        });
    }
    if !sessions.is_empty() && server_version.is_none() {
        return Err(TmuxError::Protocol);
    }
    Ok(ProbeResult {
        tmux_client_version: client_version.to_owned(),
        tmux_server_version: server_version,
        selection_epoch: Uuid::new_v4(),
        sessions,
    })
}

fn parse_marker(line: &[u8], prefix: &[u8]) -> Result<Option<(u64, u64, u64)>, TmuxError> {
    let Some(value) = line.strip_prefix(prefix) else {
        return Ok(None);
    };
    let value = value.strip_suffix(b"\n").unwrap_or(value);
    let value = std::str::from_utf8(value).map_err(|_| TmuxError::Protocol)?;
    let fields = value.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 3 {
        return Err(TmuxError::Protocol);
    }
    Ok(Some((
        fields[0].parse().map_err(|_| TmuxError::Protocol)?,
        fields[1].parse().map_err(|_| TmuxError::Protocol)?,
        fields[2].parse().map_err(|_| TmuxError::Protocol)?,
    )))
}

fn split_once_space(value: &[u8]) -> Result<(&[u8], &[u8]), TmuxError> {
    let separator = value
        .iter()
        .position(|byte| *byte == b' ')
        .ok_or(TmuxError::Protocol)?;
    Ok((&value[..separator], &value[separator + 1..]))
}

fn is_refresh_notification(line: &[u8]) -> bool {
    line == b"%sessions-changed"
        || line.starts_with(b"%layout-change ")
        || line.starts_with(b"%pane-mode-changed ")
        || line.starts_with(b"%session-changed ")
        || line.starts_with(b"%session-renamed ")
        || line.starts_with(b"%session-window-changed ")
        || line.starts_with(b"%subscription-changed ")
        || line.starts_with(b"%window-add ")
        || line.starts_with(b"%window-close ")
        || line.starts_with(b"%window-pane-changed ")
        || line.starts_with(b"%window-renamed ")
}

fn is_ignorable_notification(line: &[u8]) -> bool {
    line.starts_with(b"%client-detached ")
        || line.starts_with(b"%client-session-changed ")
        || line.starts_with(b"%message ")
        || line.starts_with(b"%paste-buffer-changed ")
        || line.starts_with(b"%paste-buffer-deleted ")
        || line.starts_with(b"%unlinked-window-add ")
        || line.starts_with(b"%unlinked-window-close ")
        || line.starts_with(b"%unlinked-window-renamed ")
}

fn command_payload(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\n").unwrap_or(line)
}

fn single_line(lines: &[Vec<u8>]) -> Result<&str, TmuxError> {
    if lines.len() != 1 {
        return Err(TmuxError::Protocol);
    }
    std::str::from_utf8(&lines[0]).map_err(|_| TmuxError::Protocol)
}

fn safe_text(lines: &[Vec<u8>], max_bytes: usize) -> Result<String, TmuxError> {
    let value = single_line(lines)?;
    if value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(TmuxError::Protocol);
    }
    Ok(value.to_owned())
}

fn decode_escaped(value: &[u8]) -> Result<Vec<u8>, TmuxError> {
    let mut decoded = Vec::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        if value[index] != b'\\' {
            decoded.push(value[index]);
            index += 1;
            continue;
        }
        if index + 1 >= value.len() {
            return Err(TmuxError::Protocol);
        }
        if value[index + 1] == b'\\' {
            decoded.push(b'\\');
            index += 2;
            continue;
        }
        if index + 3 >= value.len()
            || !(b'0'..=b'7').contains(&value[index + 1])
            || !(b'0'..=b'7').contains(&value[index + 2])
            || !(b'0'..=b'7').contains(&value[index + 3])
        {
            return Err(TmuxError::Protocol);
        }
        let byte = (value[index + 1] - b'0') * 64
            + (value[index + 2] - b'0') * 8
            + (value[index + 3] - b'0');
        decoded.push(byte);
        index += 4;
    }
    Ok(decoded)
}

fn validate_version(value: &str) -> Result<(), TmuxError> {
    let release = value.strip_prefix("tmux ").ok_or(TmuxError::Incompatible)?;
    if KNOWN_BAD_VERSIONS.contains(&release) {
        return Err(TmuxError::Incompatible);
    }
    let (major, minor, patch) = parse_version_release(release)?;
    if (major, minor) < (3, 2) || ((major, minor) == (3, 2) && patch.is_none()) {
        return Err(TmuxError::Incompatible);
    }
    Ok(())
}

fn parse_version_release(release: &str) -> Result<(u32, u32, Option<char>), TmuxError> {
    let (major, minor_and_patch) = release.split_once('.').ok_or(TmuxError::Incompatible)?;
    if major.is_empty() || !major.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(TmuxError::Incompatible);
    }
    let minor_length = minor_and_patch
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    if minor_length == 0 {
        return Err(TmuxError::Incompatible);
    }
    let (minor, patch) = minor_and_patch.split_at(minor_length);
    let patch = match patch.as_bytes() {
        [] => None,
        [value @ b'a'..=b'z'] => Some(char::from(*value)),
        _ => return Err(TmuxError::Incompatible),
    };
    Ok((
        major.parse().map_err(|_| TmuxError::Incompatible)?,
        minor.parse().map_err(|_| TmuxError::Incompatible)?,
        patch,
    ))
}

fn parse_positive_i64(value: &str) -> Result<i64, TmuxError> {
    let value = value.parse::<i64>().map_err(|_| TmuxError::Protocol)?;
    if value <= 0 {
        return Err(TmuxError::Protocol);
    }
    Ok(value)
}

fn parse_count(value: &str) -> Result<u32, TmuxError> {
    let value = value.parse::<u32>().map_err(|_| TmuxError::Protocol)?;
    if value > 10_000 {
        return Err(TmuxError::Protocol);
    }
    Ok(value)
}

fn parse_dimension(value: &str) -> Result<u32, TmuxError> {
    let value = value.parse::<u32>().map_err(|_| TmuxError::Protocol)?;
    if value == 0 || value > MAX_DIMENSION {
        return Err(TmuxError::Protocol);
    }
    Ok(value)
}

fn parse_coordinate(value: &str) -> Result<u32, TmuxError> {
    let value = value.parse::<u32>().map_err(|_| TmuxError::Protocol)?;
    if value >= MAX_DIMENSION {
        return Err(TmuxError::Protocol);
    }
    Ok(value)
}

fn parse_coordinate_with_bound(value: &str, exclusive_bound: u32) -> Result<u32, TmuxError> {
    let value = parse_coordinate(value)?;
    if value >= exclusive_bound {
        return Err(TmuxError::Protocol);
    }
    Ok(value)
}

fn parse_modes(values: &[&str]) -> Result<u8, TmuxError> {
    if values.len() != 7 {
        return Err(TmuxError::Protocol);
    }
    let mut modes = 0_u8;
    for (index, value) in values.iter().enumerate() {
        match *value {
            "0" => {}
            "1" => modes |= 1 << index,
            _ => return Err(TmuxError::Protocol),
        }
    }
    Ok(modes)
}

fn is_session_id(value: &str) -> bool {
    is_numeric_id(value, '$')
}

fn is_window_id(value: &str) -> bool {
    is_numeric_id(value, '@')
}

fn is_pane_id(value: &str) -> bool {
    is_numeric_id(value, '%')
}

fn is_numeric_id(value: &str, prefix: char) -> bool {
    value.strip_prefix(prefix).is_some_and(|digits| {
        !digits.is_empty() && digits.len() <= 30 && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TmuxError {
    Ssh(SshError),
    Incompatible,
    TooManySessions,
    TooManyPanes,
    ProjectionTooLarge,
    Protocol,
    Target,
    Changed,
    Deadline,
}

impl From<SshError> for TmuxError {
    fn from(value: SshError) -> Self {
        Self::Ssh(value)
    }
}

impl std::fmt::Display for TmuxError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Ssh(_) => "SSH/tmux transport failed",
            Self::Incompatible => "target tmux version is incompatible",
            Self::TooManySessions => "target has too many tmux sessions",
            Self::TooManyPanes => "selected tmux window has too many panes",
            Self::ProjectionTooLarge => "target tmux projection exceeds its bound",
            Self::Protocol => "tmux control protocol failed closed",
            Self::Target => "target tmux command failed",
            Self::Changed => "target tmux workspace changed during projection",
            Self::Deadline => "target tmux response deadline exceeded",
        })
    }
}
impl std::error::Error for TmuxError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimum_and_current_versions() {
        assert!(validate_version("tmux 3.2a").is_ok());
        assert!(validate_version("tmux 3.2b").is_ok());
        assert!(validate_version("tmux 3.3").is_ok());
        assert!(validate_version("tmux 3.5a").is_ok());
        for incompatible in [
            "tmux 3.1c",
            "tmux 3.2",
            "tmux 3.2aa",
            "tmux 3.2-a",
            "tmux next-3.5",
            "3.5a",
        ] {
            assert_eq!(
                validate_version(incompatible),
                Err(TmuxError::Incompatible),
                "{incompatible}"
            );
        }
    }

    #[test]
    fn decodes_control_octal_without_accepting_partial_escapes() {
        assert_eq!(
            decode_escaped(br"hello\040world\\").expect("decode"),
            b"hello world\\"
        );
        assert!(decode_escaped(br"bad\04").is_err());
    }

    #[test]
    fn guarded_command_payload_preserves_terminal_backslashes_and_escapes() {
        let payload = b"literal\\040 \\q trailing\\ \x1b[31mred\x1b[0m\n";
        assert_eq!(
            command_payload(payload),
            b"literal\\040 \\q trailing\\ \x1b[31mred\x1b[0m"
        );
    }

    #[test]
    fn probe_is_bounded_and_identity_based() {
        let output = ProbeOutput {
            stdout: "__OWLMUX_TMUX_CLIENT_V1__\ttmux 3.2a\n__OWLMUX_TMUX_SERVER_V1__\ttmux 3.2a\n$1:1700000000:1:2:alpha\n".to_owned(),
        };
        let result = parse_probe(&output).expect("probe");
        assert_eq!(result.sessions[0].session_id, "$1");
        assert_eq!(result.sessions[0].session_created, 1_700_000_000);
        assert_eq!(result.sessions[0].attached_client_count, 1);
        assert_eq!(result.sessions[0].window_count, 2);
        assert_eq!(result.tmux_server_version.as_deref(), Some("tmux 3.2a"));
    }

    #[test]
    fn probe_allows_an_absent_server_only_with_no_sessions() {
        let output = ProbeOutput {
            stdout: "__OWLMUX_TMUX_CLIENT_V1__\ttmux 3.5\n__OWLMUX_TMUX_SERVER_V1__\tnone\n"
                .to_owned(),
        };
        let result = parse_probe(&output).expect("zero-session probe");
        assert!(result.sessions.is_empty());
        assert!(result.tmux_server_version.is_none());
    }

    #[test]
    fn response_markers_must_be_exact() {
        assert_eq!(
            parse_marker(b"%begin 1700000000 42 1\n", b"%begin ").expect("marker"),
            Some((1_700_000_000, 42, 1))
        );
        assert!(parse_marker(b"%end 1 2\n", b"%end ").is_err());
    }

    #[test]
    fn pane_capture_cutover_discards_only_covered_output() {
        let mut events = VecDeque::from([
            ControlEvent::Output {
                pane_id: "%1".to_owned(),
                data: b"covered".to_vec(),
            },
            ControlEvent::Output {
                pane_id: "%2".to_owned(),
                data: b"other-pane".to_vec(),
            },
            ControlEvent::Refresh,
        ]);
        let queued = discard_pre_capture_output(&mut events, "%1");
        assert_eq!(queued, b"other-pane".len());
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events.front(),
            Some(ControlEvent::Output { pane_id, data })
                if pane_id == "%2" && data == b"other-pane"
        ));
        assert!(matches!(events.back(), Some(ControlEvent::Refresh)));
    }

    #[test]
    fn response_budget_rejects_an_empty_record_flood() {
        let mut budget = ResponseBudget::default();
        for _ in 0..MAX_RESPONSE_RECORDS {
            budget.observe(b"\n").expect("record within bound");
        }
        assert_eq!(budget.observe(b"\n"), Err(TmuxError::Protocol));
    }

    #[tokio::test]
    async fn missing_response_completion_hits_the_command_deadline() {
        let pending = std::future::pending::<Result<(), TmuxError>>();
        assert_eq!(
            response_deadline(Duration::from_millis(1), pending).await,
            Err(TmuxError::Deadline)
        );
    }

    #[test]
    fn pane_bootstrap_restores_screen_without_an_extra_row() {
        let pane = PaneProjection {
            pane_id: "%1".to_owned(),
            active: true,
            width: 8,
            height: 2,
            left: 0,
            top: 0,
            title: String::new(),
            current_command: String::new(),
            terminal: PaneTerminalState {
                cursor_x: 2,
                cursor_y: 1,
                modes: MODE_ALTERNATE | MODE_CURSOR_VISIBLE | MODE_WRAP,
                scroll_upper: 0,
                scroll_lower: 1,
            },
        };
        let lines = vec![b"abc".to_vec(), b"def".to_vec()];
        let bootstrap = capture_lines(&lines, &pane).expect("bootstrap");
        assert!(bootstrap.starts_with(b"\x1bc\x1b[?1049h"));
        assert!(bootstrap.windows(8).any(|window| window == b"abc\r\ndef"));
        assert!(!bootstrap.windows(5).any(|window| window == b"def\r\n"));
        assert!(bootstrap.windows(6).any(|window| window == b"\x1b[2;3H"));
        assert!(bootstrap.ends_with(b"\x1b[?25h"));
    }

    #[test]
    fn terminal_output_can_be_split_without_utf8_assumptions() {
        let bytes = vec![0xff; ATTACHMENT_MAX_TERMINAL_CHUNK_BYTES * 2 + 7];
        let chunks = bytes
            .chunks(ATTACHMENT_MAX_TERMINAL_CHUNK_BYTES)
            .collect::<Vec<_>>();
        assert_eq!(
            chunks.iter().map(|chunk| chunk.len()).collect::<Vec<_>>(),
            vec![16384, 16384, 7]
        );
    }
}
