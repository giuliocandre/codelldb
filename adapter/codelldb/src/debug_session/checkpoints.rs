use crate::debug_session::DebugSession;
use lldb::*;
use std::collections::{HashMap, HashSet, VecDeque};
use adapter_protocol::*;
use crate::prelude::*;
use std::cell::RefCell;
use crate::expressions::{self, FormatSpec, PreparedExpression};

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
}

impl DebugSession {
    pub(super) fn should_create_checkpoint_event(&self, process_event: &SBProcessEvent) -> bool {
        if process_event.process_state() != ProcessState::Stopped {
            return false;
        }

        let process = self.target.process();
        let thread = process.selected_thread();

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

        // Check if the faulting address is in a watched page
        let aligned_addr = fault_address & !0xFFF;
        self.checkpoints.borrow().watch_pages.contains(&aligned_addr)
    }

    pub fn mprotect_memory(&self, address: u64, protection: u8) -> Result<(), Error> {
        let process = self.target.process();
        let thread = process.selected_thread();
        let frame = thread.frame_at_index(0);

        // TODO: dirty hack with expression evaluation
        let expression = format!("(int)mprotect({}, {}, 0x1000)", address, protection);
        let (pp_expr, _) =
            expressions::prepare_with_format(&expression, self.default_expr_type).map_err(blame_user)?;

        self.evaluate_expr_in_frame(&pp_expr, Some(&frame))?;
        Ok(())
    }

    pub(super) fn add_watch_page(&mut self, address: u64) {
        // Add the address to the watch list
        let mut checkpoints = self.checkpoints.borrow_mut();
        let aligned_addr = address & !0xFFF;
        checkpoints.watch_pages.insert(address);
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

        let checkpoint = Checkpoint {
            pc: frame.pc_address().load_address(&self.target),
            frames: thread.frames().collect(),
            registers: frame.registers(),
            last_access: Some(fault_address),
        };

        self.checkpoints.borrow_mut().checkpoints.push(checkpoint);

        self.mprotect_memory(aligned_addr, 0x3)?;

        if let Err(e) = thread.step_instruction(true) {
            self.console_error(format!("Failed to step instruction: {}", e));
            return Err(e.into())
        }

        self.mprotect_memory(aligned_addr, 0x1)?;


        // Continue execution without sending StoppedEvent
        if let Err(e) = process.resume() {
            self.console_error(format!("Failed to continue execution: {}", e));
            return Err(e.into())
        }
        Ok(())
    }
}
