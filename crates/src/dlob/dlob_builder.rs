use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    dlob::dlob::DLOB, slot_subscriber::SlotSubscriber, usermap::GlobalUserMap as UserMap, SdkResult,
};

pub struct DLOBBuilder {
    slot_subscriber: SlotSubscriber,
    usermap: UserMap,
    rebuild_frequency: u64,
    dlob: DLOB,
}

impl DLOBBuilder {
    pub const SUBSCRIPTION_ID: &'static str = "dlob_update";

    pub fn new(
        slot_subscriber: SlotSubscriber,
        usermap: UserMap,
        rebuild_frequency: u64,
    ) -> DLOBBuilder {
        DLOBBuilder {
            slot_subscriber,
            usermap,
            rebuild_frequency,
            dlob: DLOB::new(),
        }
    }

    pub async fn start_building(builder: Arc<Mutex<Self>>) -> SdkResult<()> {
        let mut locked_builder = builder.lock().await;
        let rebuild_frequency = locked_builder.rebuild_frequency;
        locked_builder.slot_subscriber.subscribe(move |_slot| {})?;
        locked_builder.usermap.subscribe().await?;
        drop(locked_builder);

        tokio::task::spawn(async move {
            let mut timer =
                tokio::time::interval(tokio::time::Duration::from_millis(rebuild_frequency));
            loop {
                {
                    let mut builder = builder.lock().await;
                    builder.build();
                }
                let _ = timer.tick().await;
            }
        });

        Ok(())
    }

    pub fn build(&mut self) -> &DLOB {
        self.dlob
            .build_from_usermap(&self.usermap.usermap, self.slot_subscriber.current_slot());
        &self.dlob
    }

    pub fn get_dlob(&self) -> DLOB {
        self.dlob.clone()
    }
}
