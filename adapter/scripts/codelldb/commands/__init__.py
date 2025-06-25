from .debug_info import DebugInfoCommand
from .nofail import NoFailCommand

def register(debugger):
    debugger.HandleCommand('script import codelldb')
    debugger.HandleCommand('command script add -c codelldb.commands.DebugInfoCommand debug_info')
    debugger.HandleCommand('command script add -c codelldb.commands.NoFailCommand nofail')
    debugger.HandleCommand('command script add -f debugger.api.watch_page watch_page')
    debugger.HandleCommand('command script add -f debugger.api.get_checkpoints get_checkpoints')
