from lldb import SBValue
import warnings
import __main__
from typing import Any, Optional, Union
from pprint import pprint

from . import interface
from .value import Value
from .webview import Webview


def get_config(name: str, default: Any = None) -> Any:
    '''Retrieve a configuration value from the adapter settings.
        name:    Dot-separated path of the setting to retrieve.  For example, `get_config('foo.bar')`,
                 will retrieve the value of `lldb.script.foo.bar` from VSCode configuration.
        default: The default value to return if the configuration value is not found.
    '''
    internal_dict = interface.get_instance_dict(interface.current_debugger())
    settings = internal_dict['adapter_settings'].get('scriptConfig')
    for segment in name.split('.'):
        if settings is None:
            return default
        settings = settings.get(segment)
    return settings


def evaluate(expr: str, unwrap: bool = False) -> Union[Value,  SBValue]:
    '''Performs dynamic evaluation of native expressions returning instances of Value or SBValue.
        expression: The expression to evaluate.
        unwrap: Whether to unwrap the result and return it as lldb.SBValue
    '''
    frame = interface.current_frame()
    value = interface.nat_eval(frame, expr)
    return Value.unwrap(value) if unwrap else value


def wrap(obj: SBValue) -> Value:
    '''Extracts an lldb.SBValue from Value'''
    return obj if type(obj) is Value else Value(obj)


def unwrap(obj: Value) -> SBValue:
    '''Wraps lldb.SBValue in a Value object'''
    return Value.unwrap(obj)


def create_webview(html: Optional[str] = None, title: Optional[str] = None, view_column: Optional[int] = None,
                   preserve_focus: bool = False, enable_find_widget: bool = False,
                   retain_context_when_hidden: bool = False, enable_scripts: bool = False,
                   preserve_orphaned: bool = False):
    '''Create a [webview panel](https://code.visualstudio.com/api/references/vscode-api#WebviewPanel).
        html:               HTML content to display in the webview.  May be later replaced via Webview.set_html().
        title:              Panel title.
        view_column:        Column in which to show the webview.
        preserve_focus:     Whether to preserve focus in the current editor when revealing the webview.
        enable_find_widget: Controls whether the find widget is enabled in the panel.
        retain_context_when_hidden: Controls whether the webview panel retains its context when hidden.
        enable_scripts:     Controls whether scripts are enabled in the webview.
        preserve_orphaned:  Preserve webview panel after the end of the debug session.
    '''
    debugger_id = interface.current_debugger().GetID()
    webview = Webview(debugger_id)
    interface.send_message(debugger_id,
                           dict(message='webviewCreate',
                                id=webview.id,
                                html=html,
                                title=title,
                                viewColumn=view_column,
                                preserveFocus=preserve_focus,
                                enableFindWidget=enable_find_widget,
                                retainContextWhenHidden=retain_context_when_hidden,
                                enableScripts=enable_scripts,
                                preserveOrphaned=preserve_orphaned,
                                )
                           )
    return webview

def watch_address(addr: int):
    debugger_id = interface.current_debugger().GetID()
    interface.fire_event(debugger_id, dict(type='DebuggerMessage', address=addr))


def debugger_message(output: str, category: str = 'console'):
    debugger_id = interface.current_debugger().GetID()
    interface.fire_event(debugger_id, dict(type='DebuggerMessage', output=output, category=category))


def display_html(html: str, title: Optional[str] = None, position: Optional[int] = None, reveal: bool = False,
                 preserve_orphaned: bool = True):
    '''Display HTML content in a webview panel.
       display_html is **deprecated**, use create_webview instead.
    '''
    inst_dict = interface.get_instance_dict(interface.current_debugger())
    html_webview = inst_dict.get('html_webview')
    if html_webview is None:
        warnings.warn("display_html is deprecated, use create_webview instead", DeprecationWarning)

        html_webview = create_webview(
            html=html,
            title=title,
            view_column=position,
            preserve_focus=not reveal,
            enable_scripts=True,
            preserve_orphaned=preserve_orphaned,
        )

        def on_message(message):
            if message['command'] == 'execute':
                interface.current_debugger().HandleCommand(message['text'])

        def on_disposed(message):
            del globals()['html_webview']

        html_webview.on_did_receive_message.add(on_message)
        html_webview.on_did_dispose.add(on_disposed)
        inst_dict['html_webview'] = html_webview
    else:
        html_webview.set_html(html)
        if reveal:
            html_webview.reveal(view_column=position)



def _watch_page(addr: int):
    debugger_id = interface.current_debugger().GetID()
    interface.fire_event(debugger_id, dict(type='WatchCommand', address=addr))

def watch_page(debugger, command, result, internal_dict):
    try:
        target = debugger.GetSelectedTarget()
        process = target.GetProcess()
        args = command.strip().split()
        if len(args) != 1:
            result.SetError("Usage: watch_page <address>")
            return
        addr = target.EvaluateExpression(args[0]).GetValueAsUnsigned()
        _watch_page(addr)
    except Exception as e:
        result.SetError(str(e))

def get_checkpoint_by_access(addr: int):
    debugger_id = interface.current_debugger().GetID()
    interface.fire_event(debugger_id, dict(type='GetCheckpointByAccess', last_access=addr))

class CBManager:
    def __init__(self):
        self._callbacks = {}
        self.idx = 0
        self.wv = None

    def create_webview(self):
        webview = Webview(self.debugger_id)
        self.wv = webview

        def on_disposed(msg):
            self.wv = None

        # self.wv.on_did_receive_message.add(on_message)
        self.wv.on_did_dispose.add(on_disposed)
        html = '''
            <!DOCTYPE html>
            <html lang="en">
            <head>
            <meta charset="UTF-8" />
            <title>Checkpoints </title>
            <style>
                table {
                width: 100%;
                border-collapse: collapse;
                font-family: sans-serif;
                }
                th, td {
                padding: 8px;
                border: 1px solid #ccc;
                }
                .toggle {
                cursor: pointer;
                font-weight: bold;
                }
                .frames-row {
                display: none;
                }
                .frames-cell {
                padding-left: 40px;
                font-family: monospace;
                white-space: pre;
                }
            </style>
            </head>
            <body>
            <form id="filter-form" style="margin-bottom: 16px;">
                <label for="last-access-input">Filter by last_access (hex): </label>
                <input type="text" id="last-access-input" placeholder="e.g. 0x1234abcd" />
                <button type="submit">Filter</button>
                <button type="button" id="reset-btn">Reset</button>
            </form>
            <table id="data-table">
                <thead>
                <tr>
                    <th></th>
                    <th>Last Access</th>
                    <th>PC</th>
                    <th>File Address</th>
                </tr>
                </thead>
                <tbody></tbody>
            </table>

            <script>
                var jsonData = [];

                const tbody = document.querySelector('#data-table tbody');

                function renderTable(data) {
                tbody.innerHTML = '';
                data.forEach((item, index) => {
                    // Main row
                    const tr = document.createElement('tr');
                    let toggleTd = document.createElement('td');
                    let a = document.createElement('a');
                    toggleTd.className = 'toggle';
                    a.textContent = '▶';
                    a.id = `toggle-${index}`;
                    toggleTd.appendChild(a);
                    tr.appendChild(toggleTd);

                    tr.innerHTML += `
                    <td>${'0x' + item.last_access.toString(16)}</td>
                    <td>${'0x' + item.frames[0].load_address.toString(16)}</td>
                    <td>${item.frames[0].module} + ${'0x' + item.frames[0].file_address.toString(16)}</td>
                    `;
                    tbody.appendChild(tr);

                    document.getElementById(a.id).onclick = () => {
                    const details = document.getElementById(`frames-${index}`);
                    const expanded = details.style.display === 'table-row';
                    details.style.display = expanded ? 'none' : 'table-row';
                    document.getElementById(`toggle-${index}`).textContent = expanded ? '▶' : '▼';
                    };

                    // Expandable row with frame details
                    const framesTr = document.createElement('tr');
                    framesTr.id = `frames-${index}`;
                    framesTr.className = 'frames-row';

                    const frameDetails = item.frames.map(frame => {
                    return `[${frame.module} + 0x${frame.file_address.toString(16)}] 0x${frame.load_address.toString(16)}`
                    }).join('\\n');

                    const frameTd = document.createElement('td');
                    frameTd.colSpan = 4;
                    frameTd.className = 'frames-cell';
                    frameTd.textContent = frameDetails;

                    framesTr.appendChild(frameTd);
                    tbody.appendChild(framesTr);
                });
                }


                // Filtering logic
                document.getElementById('filter-form').addEventListener('submit', function(e) {
                e.preventDefault();
                const input = document.getElementById('last-access-input').value.trim();
                if (!input) return renderTable(jsonData);
                let searchValue = parseInt(input, 16);

                if (isNaN(searchValue)) {
                    alert('Invalid hex or decimal number');
                    return;
                }
                const filtered = jsonData.filter(item =>
                    item.last_access <= searchValue && searchValue < item.last_access + 16
                );
                renderTable(filtered);
                });

                document.getElementById('reset-btn').addEventListener('click', function() {
                document.getElementById('last-access-input').value = '';
                renderTable(jsonData);
                });

                window.addEventListener('message', event => {
                    console.log('Received message:', event.data);
                    if (event.data && event.data.type === 'checkpointData') {
                        jsonData = event.data.json;
                        renderTable(event.data.json);
                    }
                });
            </script>
            </body>
            </html>
        '''


        interface.send_message(self.debugger_id,
                        dict(message='webviewCreate',
                            id=webview.id,
                            html=html,
                            title='Checkpoints',
                            viewColumn=2,
                            preserveFocus=False,
                            enableFindWidget=True,
                            retainContextWhenHidden=True,
                            enableScripts=True,
                            preserveOrphaned=True,
                            )
                        )
        self.wv.reveal(view_column=2)


    def update_webview(self, data):
        interface.fire_event(self.debugger_id, dict(type='DebuggerMessage', output=data, category='python'))
        if not self.wv:
            self.create_webview()

        self.wv.post_message(dict(type='checkpointData', json=data))


    def new_cb(self):
        # Must be called from a valid debugger context
        self.debugger_id = interface.current_debugger().GetID()
        idx = self.idx
        def _debug_message(msg):
            checkpoints = msg['checkpoints']
            self.update_webview(checkpoints)
            self.remove_cb(idx)

        self._callbacks[idx] = _debug_message
        self.idx += 1

        return _debug_message

    def remove_cb(self, idx):
        interface.on_did_receive_message.remove(self._callbacks[idx])
        self._callbacks.pop(idx)

cb_manager = CBManager()

def get_checkpoints(debugger, command, result, internal_dict):

    debugger_id = interface.current_debugger().GetID()
    interface.on_did_receive_message.add(cb_manager.new_cb())
    interface.fire_event(debugger_id, dict(type='GetCheckpoints'))

def __lldb_init_module(debugger, internal_dict):  # pyright: ignore
    debugger.HandleCommand('command script add -c debugger.DebugInfoCommand debug_info')
    debugger.HandleCommand('command script add -f debugger.api.watch_page watch_page')
