use minhook::MinHook;

use std::{string::FromUtf8Error, char, fs::{File, remove_file, OpenOptions}, io::{Write, Take, stdout, stdin, Read}, path::Path, thread::sleep, time::Duration, ptr::{self, null}, mem, os::windows::ffi::EncodeWide};

use wchar::{wchar_t, wchz};

use toy_arms::{internal::{self, module::Module, GameObject, cast}, derive::GameObject, utils::{detect_keydown, keyboard::VirtualKeyCode}};

internal::create_entrypoint!(main_thread);

const GNAMES_OFFSET: usize = 0x3515230;
const GOBJECTS_OFFSET: usize = 0x35152D8;
const PROCESSEVENT_OFFSET: usize = 0x109ca0;
const STATICCONSTRUCTOBJECT_OFFSET: usize = 0x008c050;
const ENGINEPROCESSCOMMAND_OFFSET: usize = 0x01fca00;
const ERROR_FUNC_OFFSET: usize = 0x00646c0;
const INIT_LISTEN_OFFSET: usize = 0x0c21e30;

static mut orig_processevent_addr: usize = 0;
static mut orig_staticcreateobject_addr: usize = 0;
static mut orig_engine_exec_addr: usize = 0;
static mut orig_error_func_addr: usize = 0;

static mut engine_addr: usize = 0;
static mut foutputdevice: usize = 0;
static mut module_base_global: usize = 0;
static mut static_construct_object_param_9: usize = 0;

#[derive(GameObject)]
struct TArray {
    pointer: *const usize
}

impl TArray {
    unsafe fn get(&self, idx: usize) -> usize {
        return *cast!(self.pointer as usize + (0x8 * idx), usize);
    }
}

macro_rules! function_from_address {
    ($address:expr, $t:ty) => {
        std::mem::transmute::<*const (), $t>($address as _)
    };
}

unsafe fn get_fname_from_gnames_at_idx(gnames: *mut TArray, idx: usize) -> Option<String>{
    if((*gnames).get(idx) == 0){
        return None;
    }

    let mut out_string:String = String::new();

    for i in 0..64{
        let byte: u8 = *cast!((*gnames).get(idx) + i + 0x18, u8);
        let the_char: char = char::from_u32(byte as u32).unwrap();
        if the_char == char::from_u32(0).unwrap() {
            break;
        }
        out_string.push(the_char);
    }

    return Some(out_string);
}

struct UObject{
    address: usize,
    name_index: u32,
    name: String,
    class_name: Option<String>
}

unsafe fn get_outer_uobject_name(gnames: *mut TArray, uobject_address: usize) -> String{
    let mut str_to_return: String = String::new();

    let name_index: u32 = *cast!(uobject_address + 0x40, u32);

    str_to_return.push_str(&get_fname_from_gnames_at_idx(gnames, name_index as usize).unwrap());
    str_to_return.push_str(".");

    let outer_address: usize = *cast!(uobject_address + 0x38, usize);

    if(outer_address != 0){
        str_to_return.push_str(&get_outer_uobject_name(gnames, outer_address));
    }

    return str_to_return;
}

unsafe fn get_uobject_from_address(gnames: *mut TArray, uobject_address: usize, module_base: usize, dont_recurse: bool) -> Option<UObject>{
    if(uobject_address == 0){
        return None;
    }

    let name_index: u32 = *cast!(uobject_address + 0x40, u32);

    let mut class_name: Option<String> = None;

    if(!dont_recurse){
        class_name = Some(get_uobject_from_address(gnames, *cast!(uobject_address + 0x48, usize), module_base, true).unwrap().name);
    }

    let mut name: String = String::new();

    let outer_address: usize = *cast!(uobject_address + 0x38, usize);

    if(outer_address != 0){
        name.push_str(&get_outer_uobject_name(gnames, outer_address));
    }

    name.push_str(&get_fname_from_gnames_at_idx(gnames, name_index as usize).unwrap());

    return Some(UObject { address: uobject_address, name_index: name_index, name: name, class_name: class_name});
}

unsafe fn get_uobject_from_gobjobjects_at_idx(gnames: *mut TArray, idx: usize, gobjects: *mut TArray, module_base: usize) -> Option<UObject>{
    let uobject_address: usize = (*gobjects).get(idx);

    return get_uobject_from_address(gnames, uobject_address, module_base, false);
}

unsafe fn dump_names(gnames: *mut TArray, module: &Module){
    
    let mut names_string = String::new();

    let mut invalid_count: i32 = 0;
    let mut i = 0;

    loop{
        let maybe_name: Option<String> = get_fname_from_gnames_at_idx(gnames, i);

        if(maybe_name.is_some()){
            invalid_count = 0;
            names_string.push_str("[");
            names_string.push_str(&i.to_string());
            names_string.push_str("] ");
            names_string.push_str(&maybe_name.unwrap());
            names_string.push_str("\n");
        }
        else{
            invalid_count = invalid_count + 1;
            if(invalid_count > 10000){
                break;
            }
        }

        i = i + 1;
    }
}


#[repr(C, packed)]
struct SetFOVParams{
    fov: f32
}

#[repr(C, packed)]
struct SetSensitivityParams{
    X: f32,
    Y: f32
}

/**
 * Gets the currently instantiated PoplarCamera UObject
 */
unsafe fn get_camera(parsed_gobjects: &'static Vec<UObject>) -> Option<&'static UObject>{
    for uobject in parsed_gobjects{
        if uobject.class_name.clone().unwrap() == "PoplarGame.PoplarCamera".to_string(){
            if uobject.name.contains("PersistentLevel.TheWorld."){
                println!("{} {}", uobject.class_name.clone().unwrap(), uobject.name);
                return Some(uobject);
            }
        }
    }

    return None;
}

/**
 * Gets the currently instantiated PoplarPlayerInput UObject
 */
unsafe fn get_input(parsed_gobjects: &'static Vec<UObject>) -> Option<&'static UObject>{
    //[2a36c4a9850] [PoplarGame.PoplarPlayerInput] PoplarPlayerController.PersistentLevel.TheWorld.Slums_P.PoplarPlayerInput

    for uobject in parsed_gobjects{
        if uobject.class_name.clone().unwrap() == "PoplarGame.PoplarPlayerInput".to_string(){
            if uobject.name.contains("PoplarPlayerController.PersistentLevel.TheWorld."){
                println!("{} {}", uobject.class_name.clone().unwrap(), uobject.name);
                return Some(uobject);
            }
        }
    }

    return None;
}

/**
 * Sets the mouse sensitivity, relies on the parsed_gobjects being refreshed post level change
 */
unsafe fn set_mouse_sensitivity(parsed_gobjects: &'static Vec<UObject>, x: f32, y: f32){
    let camera_uobject = get_input(parsed_gobjects).unwrap().address;

    let fov_ufunction: usize = get_uobject_from_vec("PlayerInput.Engine.SetSensitivity".to_string(), Some("Core.Function".to_string()), parsed_gobjects).unwrap().address;

    let params: SetSensitivityParams = SetSensitivityParams { X: x, Y: y };

    println!("Changing Sensitivity to X: {:?} Y: {:?} with addrs {:x} {:x} {:x}", x, y, camera_uobject, fov_ufunction, ptr::addr_of!(params) as usize);

    fake_process_event(camera_uobject, fov_ufunction, ptr::addr_of!(params) as usize);
}

/**
 * Sets the FOV of the currently active PlayerController, must be called after each level load
 */
unsafe fn set_fov(parsed_gobjects: &Vec<UObject>, fov: f32){
    let camera_uobject = get_player_controller_address(parsed_gobjects).unwrap();

    /*
    [287b503ca48] [Core.Function] PlayerController.Engine.SetFOV
[287b503cb80] [Core.FloatProperty] SetFOV.PlayerController.Engine.NewFOV */

    let fov_ufunction: usize = get_uobject_from_vec("PlayerController.Engine.FOV".to_string(), Some("Core.Function".to_string()), parsed_gobjects).unwrap().address;

    let params: SetFOVParams = SetFOVParams { fov: fov };

    println!("Changing FOV to {:?} with addrs {:x} {:x} {:x}", fov, camera_uobject, fov_ufunction, ptr::addr_of!(params) as usize);

    fake_process_event(camera_uobject, fov_ufunction, ptr::addr_of!(params) as usize);
}

#[derive(GameObject)]
struct SetShowSubtitlesParams{
    showSubtitles: bool
}

/**
 * Sets the subtitle state of the currently active PlayerController, must be called after each level load
 */
unsafe fn set_subtitle_state(parsed_gobjects: &Vec<UObject>, enabled: bool){
    let camera_uobject = get_player_controller_address(parsed_gobjects).unwrap();
    /*
    [287b503ca48] [Core.Function] PlayerController.Engine.SetFOV
[287b503cb80] [Core.FloatProperty] SetFOV.PlayerController.Engine.NewFOV */

    let fov_ufunction: usize = get_uobject_from_vec("PlayerController.Engine.SetShowSubtitles".to_string(), Some("Core.Function".to_string()), parsed_gobjects).unwrap().address;

    let params: SetShowSubtitlesParams = SetShowSubtitlesParams { showSubtitles: enabled };

    println!("Setting subtitles to {:?} with addrs {:x} {:x} {:x}", enabled, camera_uobject, fov_ufunction, ptr::addr_of!(params) as usize);

    fake_process_event(camera_uobject, fov_ufunction, ptr::addr_of!(params) as usize);
}

/**
 * WEE WOO WEE WOO BAD CODE ALERT
 * This parses the memory containing UObjects into a Vec of the UObject struct
 * This has the awful side effect of needing to be refreshed, as it is not automatically synchronized with the game state
 * This also has the awful side effect of duplicating a TON of memory, which while probably not catastrophic in the long run is just bad programming practice
 * In the future this will be replaced with proper methods that interpret memory, instead of parsing the whole thing into a Vec
 */
unsafe fn parse_uobjects(gnames: *mut TArray, module: &Module, gobjects: *mut TArray) -> Vec<UObject>{
    let mut uobjects: Vec<UObject> = Vec::new();

    let mut names_string = String::new();

    let mut invalid_count: i32 = 0;
    let mut i = 0;

    loop{
        let maybe_object: Option<UObject> = get_uobject_from_gobjobjects_at_idx(gnames, i, gobjects, module.base_address);

        if(maybe_object.is_some()){
            let object: UObject = maybe_object.unwrap();
            invalid_count = 0;

            names_string.push_str("[");
            names_string.push_str(&format!("{:x}", &object.address));
            names_string.push_str("] ");
            names_string.push_str("[");
            if(object.class_name.is_some()){
                names_string.push_str(&format!("{}", object.class_name.clone().unwrap()));
            }
            names_string.push_str("] ");
            names_string.push_str(&object.name);
            names_string.push_str("\n");

            uobjects.push(object);
        }
        else{
            invalid_count = invalid_count + 1;
            if(invalid_count > 100){
                break;
            }
        }

        i = i + 1;
    }

    return uobjects;
}

/**
 * WEE WOO WEE WOO BAD CODE ALERT
 * This searches the provided parsed uobject vec for a UObject with a given name and optional class
 * Not only is this inflexible for supporting more advanced search patterns, it has all the downsides of the aforementioned parsed uobjects vec
 */
unsafe fn get_uobject_from_vec(name: String, class: Option<String>, vec: &Vec<UObject>) -> Option<&UObject>{
    for uobject in vec{
        if(class.is_some()){
            if(uobject.name == name){
                if(uobject.class_name.is_some()){
                    if(uobject.class_name.clone().unwrap() == class.clone().unwrap()){
                        return Some(uobject);
                    }
                }
            }
        }
        else{
            if(uobject.name == name){
                return Some(uobject);
            }
        }
    }

    return None;
}

/**
 * WEE WOO WEE WOO BAD CODE ALERT
 * The worst offender of them all, this method needs to be replaced ASAP with something that just reads the memory at the address 4head
 */
fn get_uobject_from_vec_by_address(uobject_address: usize, vec: &Vec<UObject>) -> Option<&UObject>{
    for uobject in vec{
        if(uobject.address == uobject_address){
            return Some(&uobject);
        }
    }

    return None;
}

/**
 * This is the function that is called whenever the original processevent is called
 * This function intercepts the params of process_event, fires actions based on the state that the game is put into, then calls the original process_event function
 */
unsafe fn fake_process_event(uobject_address: usize, ufunction_address: usize, params: usize) -> usize{
    type ProcessEvent = unsafe extern "thiscall" fn(uobject: usize, ufunction: usize, params: usize) -> usize;

    let process_event: ProcessEvent = unsafe { std::mem::transmute(orig_processevent_addr)};

    return process_event(uobject_address, ufunction_address, params);
}

struct ConsoleCommandParams{
    command: usize
}

#[derive(GameObject)]
struct TcpListenParams{
    returnval: usize
}

/**
 * This is the function that is called whenever the original static_construct_object is called
 * This function intercepts the params of static_construct_object, fires actions based on the state that the game is put into, then calls the original static_construct_object function
 */
unsafe fn fake_static_construct_object(param1: usize, param2: usize, param3: usize, param4: usize, param5: usize, param6: usize, param7: usize, param8: usize, param9: usize) -> usize{    
    type StaticConstructObject = unsafe extern "thiscall" fn(param1: usize, param2: usize, param3: usize, param4: usize, param5: usize, param6: usize, param7: usize, param8: usize, param9: usize) -> usize;

    let static_construct_object: StaticConstructObject = unsafe { std::mem::transmute(orig_staticcreateobject_addr)};

    return static_construct_object(param1, param2, param3, param4, param5, param6, param7, param8, param9);
}

/**
 * This is the function that is called whenever the original exec function of the GameEngine UObject is called
 * This function intercepts the params of engine_exec, fires actions based on the state that the game is put into, then calls the original engine_exec function
 */
unsafe fn fake_engine_exec(game_engine_address: usize, command: usize, f_output_device: usize) -> i32{
    type EngineCallCommand = unsafe extern "thiscall" fn(game_engine_address: usize, command: usize, f_output_device: usize) -> i32;

    let engine_call_command: EngineCallCommand = unsafe{ std::mem::transmute(orig_engine_exec_addr)};

    engine_addr = game_engine_address;
    foutputdevice = f_output_device;

    return engine_call_command(game_engine_address, command, f_output_device);
}

#[derive(GameObject)]
struct ScuffedTArray{
    pointer: usize,
    num: u32,
    count: u32
}

#[derive(GameObject)]
struct ReturnToMenuParams{
    reason: usize
}

struct NavToURLParams{
    URL: usize,
    error: usize,
    returnval: bool
}

struct TcpLinkListenParams{
    returnval: usize
}

struct ClientTravelParams{
    URL: usize,
    travelType: u8,
    bSeamless: u64,
    mapPackageGUID: usize
}

struct SetFrontendStateParams{
    state: u8
}

struct SendPlayerToURLParams{
    playerController: usize,
    URL: usize
}

struct SetClassParams{
    class: usize
}

struct ServerSelectCharacterParams{
    character: usize,
    skin: usize,
    taunt: usize
}

/**
 * Gets the currently instantiated PoplarPlayerController UObject
 */
unsafe fn get_player_controller_address(parsed_gobjects: &Vec<UObject>) -> Option<usize>{
    for uobject in parsed_gobjects{
        if(uobject.name.contains("PoplarPlayerController") && uobject.name.contains("PersistentLevel.TheWorld") && uobject.class_name == Some("PoplarGame.PoplarPlayerController".to_string())){
            println!("{}", uobject.name);
            return Some(uobject.address);
        }
    }

    return None;
}

#[derive(GameObject)]
struct CreateNamedNetDriverParams{
    name: usize
}

#[derive(GameObject)]
struct FString{
    body: usize,
    len: u32,
    max: u32
}

#[repr(C, packed)]
struct FStringBody{
    body: [usize]
}

fn main_thread() {
    println!("ReBorn Injected!");

    println!("Waiting for module to become valid...");
    loop{
        if Module::from_name("Battleborn.exe").is_some(){
            break;
        }
    }
    println!("Module valid! Continuing...");
    let module: Module = Module::from_name("Battleborn.exe").unwrap();

    let module_base_address: usize = module.base_address;

    println!("Module base address: {:x}", module_base_address);

    unsafe{
        module_base_global = module_base_address;

        let gnames: *mut TArray = TArray::from_raw(module.read(GNAMES_OFFSET)).unwrap();
        let gobjects: *mut TArray = TArray::from_raw(module.read(GOBJECTS_OFFSET)).unwrap();

        println!("Dumping names...");

        dump_names(gnames, &module);

        println!("Names dump complete!");

        /*
        println!("Waiting for a few secs to hit menu to dump objects...");

        sleep(Duration::from_secs(10));
        */

        println!("Dumping objects...");

        let uobjects: Vec<UObject> = parse_uobjects(gnames, &module, gobjects);

        println!("Objects dump complete!");

        println!("Creating ProcessEvent reference...");

        type ProcessEvent = unsafe extern "thiscall" fn(uobject: usize, ufunction: usize, params: usize);

        let process_event: ProcessEvent = unsafe { std::mem::transmute(module_base_address + PROCESSEVENT_OFFSET)};

        println!("Creating ProcessEvent hook...");

        orig_processevent_addr = MinHook::create_hook(process_event as _, fake_process_event as _).unwrap() as usize;

        println!("Creating StaticConstructObject reference...");

        type StaticConstructObject = unsafe extern "fastcall" fn(param1: usize, param2: usize, param3: usize, param4: usize, param5: usize, param6: usize, param7: usize, param8: usize, param9: usize) -> usize;

        let static_construct_object: StaticConstructObject = unsafe{std::mem::transmute(module_base_address + STATICCONSTRUCTOBJECT_OFFSET)};

        println!("Creating StaticConstructObject hook...");

        orig_staticcreateobject_addr = MinHook::create_hook(static_construct_object as _, fake_static_construct_object as _).unwrap() as usize;

        println!("Creating EngineCallCommand reference...");

        type EngineCallCommand = unsafe extern "thiscall" fn(UGameEngine: usize, command: usize, foutputdevice: usize) -> i32;

        let engine_call_command: EngineCallCommand = unsafe{ std::mem::transmute(module_base_address + ENGINEPROCESSCOMMAND_OFFSET)};

        println!("Creating EngineCallCommand hook...");

        orig_engine_exec_addr = MinHook::create_hook(engine_call_command as _, fake_engine_exec as _).unwrap() as usize;

        println!("Enabling all hooks...");

        let _ = MinHook::enable_all_hooks().unwrap();

        let stdin = stdin();
        let mut stdout = stdout();

        loop{

        }
    }
}