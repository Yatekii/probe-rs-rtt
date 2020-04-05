mod app;
mod util;

use probe_rs::{config::TargetSelector, DebugProbeInfo, Probe};
use std::collections::BTreeMap;
use structopt::StructOpt;

use probe_rs_rtt::{Rtt, RttChannel};

#[derive(Debug, StructOpt)]
#[structopt(
    name = "rtthost",
    about = "A host for debugging microcontrollers using the RTT (real-time transfer) protocol."
)]
struct Opts {
    #[structopt(
        long,
        default_value = "0",
        help = "Select a specific probe. By default the first found probe will be selected."
    )]
    probe: usize,

    #[structopt(
        long,
        help = "Target chip name. Leave this unspecified to try and auto-detect the chip."
    )]
    chip: Option<String>,

    #[structopt(long = "list-probes", help = "List all the RTT channels and exit.")]
    list_probes: bool,

    #[structopt(long, help = "List all the RTT channels and exit.")]
    list: bool,

    #[structopt(
        long,
        help = "All the up channels that should be output. Default is to output all available ones."
    )]
    up: Option<Vec<usize>>,

    #[structopt(
        long,
        help = "All the down channels that should be shown. Default is to show all available ones."
    )]
    down: Option<Vec<usize>>,
}

fn main() {
    pretty_env_logger::init();

    std::process::exit(run());
}

fn run() -> i32 {
    let opts = Opts::from_args();

    let probes = Probe::list_all();

    if probes.len() == 0 {
        eprintln!("No debug probes available. Make sure your probe is plugged in, supported and up-to-date.");
        return 1;
    }

    if opts.list_probes {
        list_probes(std::io::stdout(), &probes);
        return 0;
    }

    if opts.probe >= probes.len() {
        eprintln!("Probe {} does not exist.", opts.probe);
        list_probes(std::io::stderr(), &probes);
        return 1;
    }

    let probe = match probes[opts.probe].open() {
        Ok(probe) => probe,
        Err(err) => {
            eprintln!("Failed to open the probe: {}", err);
            return 1;
        }
    };

    let target_selector = opts
        .chip
        .clone()
        .map(|t| TargetSelector::Unspecified(t))
        .unwrap_or(TargetSelector::Auto);

    let session = match probe.attach(target_selector) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("Error creating debug session: {}", err);

            if opts.chip.is_none() {
                if let probe_rs::Error::ChipNotFound(_) = err {
                    eprintln!("Hint: Use '--chip' to specify the target chip type manually");
                }
            }

            return 1;
        }
    };

    let core = match session.attach_to_core(0) {
        Ok(core) => core,
        Err(err) => {
            eprintln!("Error attaching to core 0: {}", err);
            return 1;
        }
    };

    eprintln!("Attaching to RTT...");

    let mut rtt = match Rtt::attach(&core, &session) {
        Ok(rtt) => rtt,
        Err(err) => {
            eprintln!("Error attaching to RTT: {}", err);
            return 1;
        }
    };

    if opts.list {
        println!("Up channels:");
        list_channels(rtt.up_channels());

        println!("Down channels:");
        list_channels(rtt.down_channels());

        return 0;
    }

    let channels: (Vec<usize>, Vec<usize>) = (
        opts.up
            .unwrap_or_else(|| rtt.up_channels().keys().copied().collect()),
        opts.down
            .unwrap_or_else(|| rtt.down_channels().keys().copied().collect()),
    );

    let mut app = app::App::new(rtt, channels);
    loop {
        app.poll_rtt();
        app.render();
        if app.handle_event() {
            println!("Shutting down.");
            return 0;
        };
    }
}

fn list_probes(mut stream: impl std::io::Write, probes: &Vec<DebugProbeInfo>) {
    writeln!(stream, "Available probes:").unwrap();

    for (i, probe) in probes.iter().enumerate() {
        writeln!(
            stream,
            "  {}: {} {}",
            i,
            probe.identifier,
            probe
                .serial_number
                .as_ref()
                .map(|s| &**s)
                .unwrap_or("(no serial number)")
        )
        .unwrap();
    }
}

fn list_channels(channels: &BTreeMap<usize, RttChannel>) {
    for (i, chan) in channels.iter() {
        println!(
            "  {}: {} ({} byte buffer)",
            i,
            chan.name().as_ref().map(|s| &**s).unwrap_or("(no name)"),
            chan.buffer_size()
        );
    }
}
