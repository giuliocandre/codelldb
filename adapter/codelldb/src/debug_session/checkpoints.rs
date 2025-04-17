use crate::debug_session::DebugSession;
use lldb::*;
use std::collections::HashMap;
use adapter_protocol::*;
use crate::prelude::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use crate::expressions::{self, FormatSpec, PreparedExpression};

#[derive(Clone)]
pub struct Checkpoint {
    pub pc: Address,
    pub frames: Vec<SBFrame>,
    pub registers: SBValueList,
}

pub(super) struct Checkpoints {
    pub watch_pages: Vec<Address>,
    pub checkpoints: Vec<Checkpoint>,
}

impl Checkpoints {
    pub(super) fn new() -> Self {
        Checkpoints {
            watch_pages: Vec::new(),
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
        self.checkpoints.borrow().watch_pages.iter().any(|&page| {
            // Check if the faulting address falls within the page
            // Assuming 4KB pages for now
            let page_start = page & !0xFFF;
            let page_end = page_start + 0x1000;
            fault_address >= page_start && fault_address < page_end
        })
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
        checkpoints.watch_pages.push(address);
        if let Err(e) = self.mprotect_memory(address & !0xfff, 0x1) {
            self.console_error(format!("Failed to mprotect memory: {}", e));
            return;
        }
        self.console_message(format!("Added watch on address 0x{:X}", address));
    }

    pub(super) fn new_checkpoint(&mut self) -> Result<(), Error> {

        let process = self.target.process();
        let thread = process.selected_thread();
        let frame = thread.frame_at_index(0);

        let checkpoint = Checkpoint {
            pc: frame.pc_address().load_address(&self.target),
            frames: thread.frames().collect(),
            registers: frame.registers(),
        };

        self.checkpoints.borrow_mut().checkpoints.push(checkpoint);

        let fault_address = thread.current_fault_addr().ok_or("Failed to get fault address")?;
        self.mprotect_memory(fault_address & !0xFFF, 0x3)?;

        if let Err(e) = thread.step_instruction(true) {
            self.console_error(format!("Failed to step instruction: {}", e));
            return Err(e.into())
        }

        self.mprotect_memory(fault_address & !0xFFF, 0x1)?;


        // Continue execution without sending StoppedEvent
        if let Err(e) = process.resume() {
            self.console_error(format!("Failed to continue execution: {}", e));
            return Err(e.into())
        }
        Ok(())
    }
}
