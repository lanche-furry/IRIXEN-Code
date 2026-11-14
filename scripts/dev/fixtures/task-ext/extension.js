// Probe extension: drives the two ways a build tool starts a process.
//
// Nothing here is specific to Gradle — `executeTask` and `terminal.sendText` are
// the same two calls Gradle, Maven, npm and cargo extensions all make.

const vscode = require('vscode');

function report(line) {
  // stdout is the protocol channel, so console goes to stderr on purpose.
  console.log('TASK-PROBE ' + line);
}

exports.activate = async () => {
  const ends = [];
  vscode.tasks.onDidEndTask((e) => {
    ends.push(e.execution.task.name);
    report('onDidEndTask ' + e.execution.task.name);
  });

  // 1. A terminal, the way `./gradlew build` is launched by most extensions.
  const term = vscode.window.createTerminal({ name: '构建' });
  term.sendText('echo terminal-path');

  // 2. ProcessExecution with an argv array: spaces must survive as one arg.
  const t1 = new vscode.Task(
    { type: 'mock' },
    'Build',
    'mock',
    new vscode.ProcessExecution('node', ['-e', 'console.log("argv ok")']),
  );
  const e1 = await vscode.tasks.executeTask(t1);
  report('executeTask returned ' + (e1 && e1.task ? e1.task.name : 'nothing'));

  // 3. ShellExecution given a whole command line.
  const t2 = new vscode.Task({ type: 'mock' }, 'Clean', 'mock', new vscode.ShellExecution('node --version'));
  await vscode.tasks.executeTask(t2);

  // 4. A provider, the way the Gradle extension contributes its tasks.
  vscode.tasks.registerTaskProvider('gradle', {
    provideTasks: () => [
      new vscode.Task({ type: 'gradle', task: 'assemble' }, 'assemble', 'gradle', new vscode.ShellExecution('gradle assemble')),
    ],
    resolveTask: (task) => task,
  });
  const found = await vscode.tasks.fetchTasks();
  report('fetchTasks -> ' + found.map((t) => t.name).join(', '));

  setTimeout(() => report('ends=' + ends.length), 4000);
};
