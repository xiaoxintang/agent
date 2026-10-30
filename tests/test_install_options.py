"""Exercise each real terminal question independently without installing anything."""
import errno
import fcntl
import os
from pathlib import Path
import pty
import select
import subprocess
import sys
import tempfile
import termios
import time
import unittest

ROOT=Path(__file__).resolve().parent.parent

class InstallerOptionsTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='agent-options-')
        self.addCleanup(self.temp.cleanup)
        self.binary=Path(self.temp.name)/'agent'
        self.binary.write_text(f'#!{sys.executable}\nprint("agent_port=8080\\ngrpc_port=10085\\nweb_port=3000\\nweb_enabled=no")\n')
        self.binary.chmod(0o755)

    def script(self, agent='', grpc='', web='', enabled='', noninteractive=0):
        return f'''set -eu
agent_binary='{self.binary}'
update=0 non_interactive={noninteractive}
arg_agent_port='{agent}' arg_grpc_port='{grpc}' arg_web_port='{web}' arg_web='{enabled}'
. '{ROOT}/deploy/options.sh'
resolve_options
printf '\\nRESULT:%s,%s,%s,%s\\n' "$agent_port" "$grpc_port" "$web_port" "$web_enabled"
'''

    def terminal(self, script, answers):
        master,slave=pty.openpty()
        attrs=termios.tcgetattr(slave);attrs[3]&=~termios.ECHO;termios.tcsetattr(slave,termios.TCSANOW,attrs)
        def session():
            os.setsid();fcntl.ioctl(slave,termios.TIOCSCTTY,0)
        process=subprocess.Popen(['/bin/sh','-c',script],stdin=slave,stdout=slave,stderr=slave,preexec_fn=session)
        os.close(slave)
        output=b'';deadline=time.monotonic()+10;index=0
        try:
            while time.monotonic()<deadline:
                readable,_,_=select.select([master],[],[],0.1)
                if readable:
                    try:data=os.read(master,65536)
                    except OSError as e:
                        if e.errno==errno.EIO:break
                        raise
                    if not data:break
                    output+=data
                text=output.decode(errors='replace')
                if index<len(answers) and answers[index][0] in text:
                    os.write(master,(answers[index][1]+'\n').encode());index+=1
                if process.poll() is not None and not readable:break
            process.wait(timeout=1)
        finally:
            if process.poll() is None:process.kill();process.wait()
            os.close(master)
        self.assertEqual(index,len(answers),output.decode(errors='replace'))
        return process.returncode,output.decode(errors='replace')

    def test_agent_and_grpc_have_independent_default_questions(self):
        code,out=self.terminal(self.script(),[('Agent HTTP 是否使用默认端口 8080','n'),('请输入 Agent HTTP 端口','9000'),('Xray 管理 gRPC 是否使用默认端口 10085',''),('是否启用 Web 服务','')])
        self.assertEqual(code,0,out);self.assertIn('RESULT:9000,10085,3000,no',out)
        self.assertNotIn('Web 是否使用默认端口',out)

    def test_web_port_is_asked_only_after_enable(self):
        code,out=self.terminal(self.script(),[('Agent HTTP 是否使用默认端口',''),('Xray 管理 gRPC 是否使用默认端口','n'),('请输入 Xray 管理 gRPC 端口','12000'),('是否启用 Web 服务','y'),('Web 是否使用默认端口','n'),('请输入 Web 端口','4000')])
        self.assertEqual(code,0,out);self.assertIn('RESULT:8080,12000,4000,yes',out)

    def test_explicit_port_skips_only_its_own_prompt(self):
        code,out=self.terminal(self.script(agent='9000',enabled='no'),[('Xray 管理 gRPC 是否使用默认端口','')])
        self.assertEqual(code,0,out);self.assertNotIn('Agent HTTP 是否使用默认端口',out);self.assertNotIn('是否启用 Web 服务',out)

    def test_noninteractive_defaults_and_collision(self):
        for grpc,enabled,web,success in [('', '', '',True),('8080','','',False),('','yes','8080',False),('99999','','',False)]:
            result=subprocess.run(['/bin/sh','-c',self.script(grpc=grpc,enabled=enabled,web=web,noninteractive=1)],capture_output=True,text=True)
            self.assertEqual(result.returncode==0,success,result.stdout+result.stderr)

    def test_no_terminal_requires_noninteractive_flag(self):
        result=subprocess.run(['/bin/sh','-c',self.script()],start_new_session=True,capture_output=True,text=True)
        self.assertNotEqual(result.returncode,0);self.assertIn('--non-interactive',result.stderr)

if __name__=='__main__':unittest.main()
