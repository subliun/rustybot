/*  main.rs
 *
 *
 *  Copyright (C) 2015 rustybot All Rights Reserved.
 *
 *  This file is part of rustybot.
 *
 *  rustybot is free software: you can redistribute it and/or modify
 *  it under the terms of the GNU General Public License as published by
 *  the Free Software Foundation, either version 3 of the License, or
 *  (at your option) any later version.
 *
 *  rustybot is distributed in the hope that it will be useful,
 *  but WITHOUT ANY WARRANTY; without even the implied warranty of
 *  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *  GNU General Public License for more details.
 *
 *  You should have received a copy of the GNU General Public License
 *  along with rustybot. If not, see <http://www.gnu.org/licenses/>.
 *
 */

#[macro_use(lazy_static)]
extern crate lazy_static;
extern crate rand;
extern crate time;
extern crate num;
extern crate rstox;

use std::error::Error;
use std::fs::{File, metadata};
use std::path::Path;
use std::io::prelude::*;
use std::io::BufReader;
use std::cmp::*;
use rand::*;
use time::{get_time, Duration};
use rstox::core::*;

mod trivia;
use self::trivia::*;
mod group;
use self::group::*;
mod bot;
use self::bot::*;
mod commands;
use self::commands::execute;

const QUESTIONS_PATH: &'static str = "data/questions";
const MASTERKEYS_PATH: &'static str = "data/masterkeys";
const DHT_NODES_PATH: &'static str = "data/DHTnodes";

// Time to wait between bootstrap attempts
const BOOTSTRAP_INTERVAL: i64 = 10;

// Number of random bootstrap nodes to connect to per try
const MAX_BOOTSTRAP_NODES: usize = 5;

// Use in case DHTnodes file fails to load
const BOOTSTRAP_IP: &'static str = "144.76.60.215";
const BOOTSTRAP_PORT: u16 = 33445;
const BOOTSTRAP_KEY: &'static str = "04119E835DF3E78BACF0F84235B300546AF8B936F035185E2A8E9E0A67C8924F";

fn load_tox() -> Result<Tox, String>
{
    let options = ToxOptions::new();
    let path = Path::new(PROFILE_DATA_PATH);
    let display = path.display();

    let file_exists = metadata(&path).is_ok();

    if !file_exists {
        File::create(&path).expect(&format!("Failed to create data file {}", display));

        let tox = Tox::new(options, None).map_err(|e| format!("Failed to open data file {}: {:?}", display, &e));

        return tox;
    } else {
        let fp = try!(File::open(path).map_err(|e| format!("Failed to open data file {}: {:?}", display, &e)));

        let mut buf = Vec::new();
        let mut reader = BufReader::new(&fp);

        try!(reader.read_to_end(&mut buf).map_err(|e| format!("Failed to read tox data to buffer: {}", &e)));

        let tox = Tox::new(options, Some(&mut buf)).map_err(|e| format!("Tox instance failed to initialize ({:?})", e));

        return tox;
    }
}

fn init_tox(tox: &mut Tox)
{
    if tox.get_name().is_empty() {
        match tox.set_name(NAME) {
            Ok(_)  => (),
            Err(e) => println!("Failed to set default name ({:?})", e),
        };
    }

    if tox.get_status_message().is_empty() {
        match tox.set_status_message(STATUS_MESSAGE) {
            Ok(_)  => (),
            Err(e) => println!("Failed to set default status message ({:?})", e),
        };
    }
}

fn bootstrap_backup(tox: &mut Tox)
{
    println!("Trying backup bootstrap server...");

    match tox.bootstrap(BOOTSTRAP_IP, BOOTSTRAP_PORT, BOOTSTRAP_KEY.parse().unwrap()) {
        Ok(_)  => (),
        Err(e) => println!("Failed to bootstrap with backup ({:?}).", e),
    }
}

fn bootstrap_tox(bot: &mut Bot)
{
    if get_time() - bot.last_connect < Duration::seconds(BOOTSTRAP_INTERVAL) {
        return;
    }

    bot.last_connect = get_time();
    println!("Bootstrapping to DHT network...");

    let path = Path::new(DHT_NODES_PATH);
    let display = path.display();

    let mut fp = match File::open(&path) {
        Ok(fp) => fp,
        Err(e) => {
            println!("Failed to open file {}: {}", display, Error::description(&e));
            bootstrap_backup(bot.tox);
            return;
        }
    };

    let mut nodes_str = String::new();

    match fp.read_to_string(&mut nodes_str) {
        Ok(_)  => (),
        Err(e) => {
            println!("Failed to read file {}: {}", display, Error::description(&e));
            bootstrap_backup(bot.tox);
            return;
        }
    };

    let nodes: Vec<&str> = nodes_str.split("\n").collect();
    let num_nodes = nodes.len();
    let mut rng = thread_rng();

    for _ in 0..min(MAX_BOOTSTRAP_NODES, num_nodes as usize) {
        let idx = rng.gen_range(0, num_nodes);
        let node: Vec<&str> = nodes[idx].split(" ").collect();

        if node.len() != 3 {
            continue;
        }

        let ip = node[0];

        let port = match node[1].to_string().parse::<u16>() {
            Ok(port) => port,
            Err(_)   => continue,
        };

        let key = match node[2].parse() {
            Ok(key) => key,
            Err(_)  => continue,
        };

        match bot.tox.bootstrap(ip, port, key) {
            Ok(_)  => (),
            Err(e) => println!("Bootstrap failed: {:?}", e),
        }
    }
}

fn load_trivia_questions(bot: &mut Bot) -> Result<i32, i32>   // 0 on success, -1 on error
{

    println!("Loading trivia questions...");

    let path = Path::new(QUESTIONS_PATH);
    let display = path.display();

    let mut fp = match File::open(&path) {
        Ok(fp) => fp,
        Err(e) => {
            println!("Failed to open file {}: {}", display, Error::description(&e));
            return Err(-1);
        }
    };

    let mut questions = String::new();

    match fp.read_to_string(&mut questions) {
        Ok(_)  => (),
        Err(e) => {
            println!("Failed to read file {}: {}", display, Error::description(&e));
            return Err(-1);
        }
    }

    for line in questions.split("\n") {
        bot.questions.push(line.to_string());
    }

    Ok(0)
}

// Returns true if peernumber is in the masterkeys list or is the owner of groupnumber
fn check_privilege(bot: &mut Bot, groupnumber: i32, peernumber: i32) -> bool
{
    let public_key = match get_peer_public_key(bot.tox, groupnumber, peernumber) {
        Some(key) => key.to_string(),
        None => {
            println!("Failed to fetch peer {}'s key in group {}", peernumber, groupnumber);
            return false;
        }
    };

    let path = Path::new(MASTERKEYS_PATH);
    let display = path.display();

    let mut fp = match File::open(&path) {
        Ok(fp) => fp,
        Err(e) => {
            println!("Failed to open file {}: {}", display, Error::description(&e));
            return false;
        }
    };

    let mut keys = String::new();

    match fp.read_to_string(&mut keys) {
        Ok(_)  => (),
        Err(e) => {
            println!("Failed to read file {}: {}", display, Error::description(&e));
            return false;
        }
    };

    for key in keys.split("\n\r") {
        if key.contains(&public_key) {
            return true;
        }
    }

    for g in &bot.groups {
        if g.groupnumber != groupnumber {
            continue;
        }

        if g.owner_pk == public_key {
            return true;
        }

        break;
    }

    false
}

fn cb_connection_status(bot: &mut Bot, status: Connection)
{
    match status {
        Connection::None => bot.last_connect = get_time(),
        _ => (),
    }

    println!("DHT connection status: {:?}", status);
}

fn cb_friend_request(bot: &mut Bot, id: PublicKey, msg: String)
{
    let id_string = id.to_string();
    println!("Friend request from:\n{}", id_string);
    println!("Message: {}", msg);

    match bot.tox.add_friend_norequest(&id) {
        Ok(_)  => {
            println!("Friend added.");
            bot.save();
        }
        Err(e) => println!("Failed to add friend ({:?})", e),
    };
}

fn cb_group_invite(bot: &mut Bot, friendnumber: i32, kind: GroupchatType, key: Vec<u8>)
{
    bot.add_group(friendnumber, key);
}

fn cb_group_namelist_change(bot: &mut Bot, groupnumber: i32, peernumber: i32, change: ChatChange)
{
    let index = match get_group_index(bot, groupnumber) {
        Some(index) => index,
        None        => return,
    };

    let public_key = match bot.tox.group_peer_pubkey(groupnumber, peernumber) {
        Some(key) => key.to_string(),
        None      => return println!("Failed to fetch peer {}'s public key", peernumber),
    };

    match change {
        ChatChange::PeerAdd  => bot.groups[index].add_peer(public_key),
        ChatChange::PeerName => bot.groups[index].update_name(bot.tox, peernumber, public_key),
        ChatChange::PeerDel  => {
            bot.groups[index].del_peer(public_key);

            // Leave group if empty
            let num_peers = match bot.tox.group_number_peers(groupnumber) {
                Some(num_peers) => num_peers,
                None            => return,
            };

            if num_peers <= 1 {
                bot.del_group(groupnumber);
            }
        }
    }
}

fn cb_group_message(bot: &mut Bot, groupnumber: i32, peernumber: i32, msg: String)
{
    if msg.is_empty() {
        return;
    }

    if msg.as_bytes()[0] == b'!' {
        execute(bot, groupnumber, peernumber, msg);
    } else {
        process_answer(bot, groupnumber, peernumber, msg);
    }
}

fn do_tox(bot: &mut Bot)
{
    for event in bot.tox.iter() {
        match event {
            ConnectionStatus(status) =>
                cb_connection_status(bot, status),
            FriendRequest(id, msg) =>
                cb_friend_request(bot, id, msg),
            GroupInvite(friendnumber, kind, key) =>
                cb_group_invite(bot, friendnumber, kind, key),
            GroupNamelistChange(groupnumber, peernumber, change) =>
                cb_group_namelist_change(bot, groupnumber, peernumber, change),
            GroupMessage(groupnumber, peernumber, msg) =>
                cb_group_message(bot, groupnumber, peernumber, msg),
            _ => (),
        }
    }

    bot.tox.wait();
}

fn do_connection(bot: &mut Bot)
{
    match bot.tox.get_connection_status() {
        Connection::None => bootstrap_tox(bot),
        _ => (),
    }
}
fn do_rustybot(bot: &mut Bot)
{
    do_tox(bot);
    do_trivia(bot);
    do_connection(bot);
}

fn main()
{
    let mut tox = match load_tox() {
        Ok(tox) => tox,
        Err(e)  => {
		println!("{}", e); 
		return;
	}
    };

    init_tox(&mut tox);
    let mut bot = Bot::new(&mut tox);
    bot.print_info();
    bot.save();

    match load_trivia_questions(&mut bot) {
        Ok(_)  => println!("Loaded."),
        Err(_) => println!("Trivia questions failed to load"),
    }

    loop {
        do_rustybot(&mut bot);
    }
}
