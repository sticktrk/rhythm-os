// Must run before any other matter.js imports. The converter supplies its own
// local storage and crypto services; ambient Matter settings must have no effect.
import { config } from '@matter/nodejs/config';
config.loadProcessArgv = false;
config.loadProcessEnv = false;
config.loadConfigFile = false;
config.initializeStorage = false;
config.installFilesystem = false;
config.installNetwork = false;
config.trapProcessSignals = false;
config.trapUnhandledErrors = false;
config.setProcessExitCodeOnError = false;
