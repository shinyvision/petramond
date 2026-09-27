use super::Game;
use petramond::net::protocol::{ChatLine, ClientToServer};

impl Game {
    pub fn save_all(&mut self) {
        self.net.save_all();
    }

    pub fn shutdown(mut self) {
        self.net.shutdown();
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.send_pause(paused);
    }

    fn send_pause(&mut self, paused: bool) {
        if self.net.is_remote() {
            return;
        }
        self.net.send_now(ClientToServer::Pause(paused));
    }

    pub fn send_chat(&mut self, text: String) {
        self.net.queue(ClientToServer::ChatSend { text });
    }

    pub fn set_particles_mode(&mut self, mode: petramond::save::client::ParticlesMode) {
        self.fx.particles.set_count_scale(mode.density());
    }

    pub fn set_view_distance(&mut self, chunks: i32) {
        let chunks = chunks.clamp(4, 64);
        self.replica.world.set_render_dist(chunks);
        let msg = ClientToServer::SetViewDistance {
            chunks: chunks as u8,
        };
        if self.net.is_remote() {
            self.net.queue(msg);
        } else {
            self.net.send_now(msg);
        }
    }

    pub fn take_chat_lines(&mut self) -> Vec<ChatLine> {
        std::mem::take(&mut self.replica.chat_lines)
    }

    #[inline]
    pub fn is_remote(&self) -> bool {
        self.net.is_remote()
    }

    pub fn open_to_lan(&mut self, port: u16) -> std::io::Result<u16> {
        let bound = self.net.open_to_lan(port)?;
        if let mod_api::ClientContext::Local { shared, .. } =
            &mut self.client_mods.presented().lock().context
        {
            *shared = true;
        }
        Ok(bound)
    }

    pub fn take_connection_lost(&mut self) -> Option<String> {
        self.net.take_lost_report()
    }
}
