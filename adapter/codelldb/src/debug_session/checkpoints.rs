use crate::debug_session::DebugSession;
use lldb::*;
use std::collections::{HashMap, HashSet, VecDeque};
use adapter_protocol::*;
use crate::prelude::*;
use std::cell::RefCell;
use crate::expressions::{self, FormatSpec, PreparedExpression};
use serde::{Serialize, Deserialize};

/* Checkpoints are created before the actual memory write */
#[derive(Clone)]
pub struct Checkpoint {
    pub pc: Address,
    pub last_access: Option<Address>,
    pub frames: Vec<SBFrame>,
    pub registers: SBValueList,
}

impl std::fmt::Debug for Checkpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Checkpoint")
            .field("pc", &self.pc)
            .field("last_access", &self.last_access)
            .field("frames", &format!("\n{}", self.frames.iter()
                .map(|frame| format!("{:?}", frame))
                .collect::<Vec<_>>()
                .join("\n")))
            .field("registers", &"<SBValueList>")
            .finish()
    }
}

pub(super) struct Checkpoints {
    pub watch_pages: HashSet<Address>,
    pub checkpoints: Vec<Checkpoint>,
}

impl Checkpoints {
    pub(super) fn new() -> Self {
        Checkpoints {
            watch_pages: HashSet::new(),
            checkpoints: Vec::new(),
        }
    }

    pub(super) fn find_checkpoint_by_last_access(&self, address: Address) -> Option<&Checkpoint> {
        self.checkpoints.iter().rev().find(|checkpoint| {
            checkpoint.last_access.map(|last_access| last_access == address).unwrap_or(false)
        })
    }
}

impl DebugSession {

    pub (super) fn handle_checkpoint_event(&mut self, stopped_thread: &SBThread) -> bool {
        if !self.should_create_checkpoint_event(stopped_thread) {
            // self.console_message("should_create_checkpoint_event false");
            return false;
        }

        self.new_checkpoint().is_ok()
    }

    pub(super) fn should_create_checkpoint_event(&self, stopped_thread: &SBThread) -> bool {
        let thread = stopped_thread;

        if thread.stop_reason() != StopReason::Signal {
            return false;
        }

        // Check if the signal is SIGSEGV
        let signal = thread.stop_reason_data_at_index(0);
        if signal != 11 { // SIGSEGV
            return false;
        }

        let fault_address = match thread.current_fault_addr() {
            Some(addr) => addr,
            None => return false,
        };
        self.console_message(format!("checkpoint_event fault addr {:#x}", fault_address));

        // Check if the faulting address is in a watched page
        let aligned_addr = fault_address & !0xFFF;
        self.checkpoints.borrow().watch_pages.contains(&aligned_addr)
    }

    pub fn mprotect_memory(&self, address: u64, protection: u8) -> Result<(), Error> {
        let process = self.target.process();
        let thread = process.selected_thread();
        let frame = thread.frame_at_index(0);

        // TODO: dirty hack with expression evaluation
        let expression = format!("(int)mprotect({}, 0x1000, {})", address, protection);
        let val = frame.evaluate_expression(&expression);
        if !val.is_valid() || val.value_as_signed(-1) == -1 {
            let err = format!("mprotect({:#x}, {}) : {:#?}", address, protection, val);
            self.console_error(&err);
            return Err(Error::from(err));
        }
        Ok(())
    }

    pub(super) fn add_watch_page(&mut self, address: u64) {
        // Add the address to the watch list
        let mut checkpoints = self.checkpoints.borrow_mut();
        let aligned_addr = address & 0xFFFFFFFFFFF000; // Ignore top byte and page-align
        checkpoints.watch_pages.insert(aligned_addr);
        if let Err(e) = self.mprotect_memory(aligned_addr, 0x1) {
            self.console_error(format!("Failed to mprotect memory: {}", e));
            return;
        }
        self.console_message(format!("Added watch on address 0x{:X}", address));
    }

    pub(super) fn new_checkpoint(&mut self) -> Result<(), Error> {

        let process = self.target.process();
        let thread = process.selected_thread();
        let frame = thread.frame_at_index(0);
        let fault_address = thread.current_fault_addr().ok_or("Failed to get fault address")?;
        let aligned_addr = fault_address & !0xFFF;
        let signals = process.unix_signals();

        let checkpoint = Checkpoint {
            pc: frame.pc_address().load_address(&self.target),
            frames: thread.frames().collect(),
            registers: frame.registers(),
            last_access: Some(fault_address),
        };

        self.checkpoints.borrow_mut().checkpoints.push(checkpoint);

        self.mprotect_memory(aligned_addr, 0x3)?;
        // Suppress SIGSEGV while stepping over
        signals.set_should_suppress(11, true);

        // Need the sync mode here because we want to step a single instruction without getting another
        // processs Stopped event (normally LLDB stops with StopReason::Trace)
        self.before_resume();
        if let Err(e) = self.with_sync_mode(|| {
            thread.step_instruction(true)
        }) {
            self.console_error(format!("Failed to step instruction: {}", e));
            return Err(e.into())
        }

        self.mprotect_memory(aligned_addr, 0x1)?;


        // Continue execution and reactivate SIGSEGV
        signals.set_should_suppress(11, false);

        if let Err(e) = process.resume() {
            self.console_error(format!("Failed to continue execution: {}", e));
            return Err(e.into())
        }
        Ok(())
    }

    pub(super) fn print_checkpoint_by_last_access(&mut self, address: Address) {
        if let Some(cp) = self.checkpoints.borrow().find_checkpoint_by_last_access(address) {
            self.console_message(format!("{:#?}", cp));
        }
    }

    pub(super) fn get_checkpoints(&mut self) {
        // let checkpoints = self.checkpoints.borrow().checkpoints.clone();
        self.handle_python_message(serde_json::json!({
            "type": "GetCheckpoints",
            "checkpoints": "test",
        }));
    }
}
