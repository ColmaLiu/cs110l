use nix::sys::ptrace;
use nix::sys::signal;
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;
use std::collections::HashMap;
use std::mem::size_of;
use std::os::unix::process::CommandExt;
use std::process::Child;
use std::process::Command;
use crate::debugger::Breakpoint;
use crate::dwarf_data::DwarfData;

pub enum Status {
    /// Indicates inferior stopped. Contains the signal that stopped the process, as well as the
    /// current instruction pointer that it is stopped at.
    Stopped(signal::Signal, usize),

    /// Indicates inferior exited normally. Contains the exit status code.
    Exited(i32),

    /// Indicates the inferior exited due to a signal. Contains the signal that killed the
    /// process.
    Signaled(signal::Signal),
}

/// This function calls ptrace with PTRACE_TRACEME to enable debugging on a process. You should use
/// pre_exec with Command to call this in the child process.
fn child_traceme() -> Result<(), std::io::Error> {
    ptrace::traceme().or(Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        "ptrace TRACEME failed",
    )))
}

fn align_addr_to_word(addr: usize) -> usize {
    addr & (-(size_of::<usize>() as isize) as usize)
}

pub struct Inferior {
    child: Child,
}

impl Inferior {
    /// Attempts to start a new inferior process. Returns Some(Inferior) if successful, or None if
    /// an error is encountered.
    pub fn new(target: &str, args: &Vec<String>, breakpoints: &mut HashMap<usize, Option<Breakpoint>>) -> Option<Inferior> {
        // TODO: implement me!
        let mut command = Command::new(target);
        command.args(args);
        unsafe {
            command.pre_exec(child_traceme);
        }
        match command.spawn() {
            Ok(child) => {
                let mut inferior = Inferior{child};
                for (addr, breakpoint) in breakpoints {
                    match inferior.write_byte(*addr, 0xcc) {
                        Ok(orig_byte) => {
                            *breakpoint = Some(Breakpoint{addr: *addr, orig_byte});
                        }
                        Err(err) => {
                            println!("{}", err);
                        }
                    }
                }
                Some(inferior)
            }
            Err(_) => None,
        }
    }

    pub fn continue_exec(&mut self, breakpoints: &HashMap<usize, Option<Breakpoint>>) -> Result<Status, nix::Error> {
        let mut regs = ptrace::getregs(self.pid())?;
        let instruction_ptr: usize = regs.rip as usize;
        if let Some(breakpoint) = breakpoints.get(&(instruction_ptr - 1)) {
            let breakpoint = breakpoint.as_ref().unwrap();
            self.write_byte(breakpoint.addr, breakpoint.orig_byte)?;
            regs.rip = breakpoint.addr as u64;
            ptrace::setregs(self.pid(), regs)?;
            ptrace::step(self.pid(), None)?;
            match self.wait(None)? {
                Status::Exited(exit_code) => return Ok(Status::Exited(exit_code)),
                Status::Signaled(signal) => return Ok(Status::Signaled(signal)),
                Status::Stopped(_, _) => {
                    self.write_byte(breakpoint.addr, 0xcc)?;
                }
            }
        }
        ptrace::cont(self.pid(), None)?;
        self.wait(None)
    }

    pub fn kill(&mut self) {
        println!("Killing running inferior (pid {})", self.pid());
        self.child.kill().unwrap();
        self.wait(None).unwrap();
    }

    pub fn print_backtrace(&self, debug_data: &DwarfData) -> Result<(), nix::Error> {
        let regs = ptrace::getregs(self.pid())?;
        let mut instruction_ptr: usize = regs.rip as usize;
        let mut base_ptr: usize = regs.rbp as usize;
        loop {
            let line = debug_data.get_line_from_addr(instruction_ptr).unwrap();
            let function = debug_data.get_function_from_addr(instruction_ptr).unwrap();
            println!("{} ({})", function, line);
            if function == "main" {
                break;
            }
            instruction_ptr = ptrace::read(self.pid(), (base_ptr + 8) as ptrace::AddressType)? as usize;
            base_ptr = ptrace::read(self.pid(), base_ptr as ptrace::AddressType)? as usize;
        }
        Ok(())
    }

    pub fn write_byte(&mut self, addr: usize, val: u8) -> Result<u8, nix::Error> {
        let aligned_addr = align_addr_to_word(addr);
        let byte_offset = addr - aligned_addr;
        let word = ptrace::read(self.pid(), aligned_addr as ptrace::AddressType)? as u64;
        let orig_byte = (word >> 8 * byte_offset) & 0xff;
        let masked_word = word & !(0xff << 8 * byte_offset);
        let updated_word = masked_word | ((val as u64) << 8 * byte_offset);
        unsafe {
            ptrace::write(
                self.pid(),
                aligned_addr as ptrace::AddressType,
                updated_word as *mut std::ffi::c_void,
            )?;
        }
        Ok(orig_byte as u8)
    }

    /// Returns the pid of this inferior.
    pub fn pid(&self) -> Pid {
        nix::unistd::Pid::from_raw(self.child.id() as i32)
    }

    /// Calls waitpid on this inferior and returns a Status to indicate the state of the process
    /// after the waitpid call.
    pub fn wait(&self, options: Option<WaitPidFlag>) -> Result<Status, nix::Error> {
        Ok(match waitpid(self.pid(), options)? {
            WaitStatus::Exited(_pid, exit_code) => Status::Exited(exit_code),
            WaitStatus::Signaled(_pid, signal, _core_dumped) => Status::Signaled(signal),
            WaitStatus::Stopped(_pid, signal) => {
                let regs = ptrace::getregs(self.pid())?;
                Status::Stopped(signal, regs.rip as usize)
            }
            other => panic!("waitpid returned unexpected status: {:?}", other),
        })
    }
}
