/*-
 * ========================LICENSE_START=================================
 * PREvant Frontend
 * %%
 * Copyright (C) 2018 - 2019 aixigo AG
 * %%
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 * THE SOFTWARE.
 * =========================LICENSE_END==================================
 */
import { EventSource } from 'eventsource';
import { Store } from 'vuex';
import { compareByPropertyWithKeywordFirst } from '../utils/sorting-util';
import { useConfig } from '../composables/useConfig';
const { defaultAppName } = useConfig();

const SERVICE_TYPE_ORDER = [
   'instance',
   'replica',
   'app-companion',
   'service-companion'
];

export function createStore(router, me, issuers) {
   const store = new Store({
      state: {
         fetchInProgress: false,
         apps: {},
         appsError: null,
         tickets: {},
         ticketsError: null,
         appNameFilter: '',
         me,
         issuers,
         // This set keeps all application names that are optimistically deleted
         hiddenApps: [],
         // This map keeps all applications that are optimistically in the process of being created
         appsInCreation: {},
      },
      getters: {
         me: state => state.me,
         issuers: state => state.issuers,

         appNameFilter: state => state.appNameFilter,

         hiddenApps: state => {
            return new Set(state.hiddenApps);
         },

         appsInCreation: state => state.appsInCreation,

         reviewApps: (state, getters) => {
            if (state.apps === undefined || Object.keys(state.apps).length == 0) {
               return [];
            }

            const appNames = Object.keys(state.apps);
            const apps = [...appNames, ...Array.from(new Set(Object.keys(getters.appsInCreation)).difference(new Set(appNames)))]
               .filter(app => !getters.hiddenApps.has(app))
               .map(appDetails)
               .sort(compareByPropertyWithKeywordFirst("name", defaultAppName.value));

            return apps
               .filter(app => app.name != null)
               .filter(app => !state.appNameFilter || app.name.toLocaleLowerCase().indexOf(state.appNameFilter.toLocaleLowerCase()) >= 0);

            function appDetails(name) {
               const app = state.apps[name];

               if (app == null) {
                  return {};
               }

               const ticket = state.tickets[name];
               const owners = app.owners;
               const status = app.status;

               const containers = (app.services || [])
                     .map(({ name, url, openApiUrl, asyncApiUrl, version, type, state }) => {
                        return {
                           name, url, openApiUrl, asyncApiUrl, version, type, status: state.status
                        };
                     });

               containers.sort(byTypeAndName);
               return { name, ticket, containers, owners, status };
            }

            function byTypeAndName(containerA, containerB) {
               const typeIndexA = SERVICE_TYPE_ORDER.indexOf(containerA.type);
               const typeIndexB = SERVICE_TYPE_ORDER.indexOf(containerB.type);

               if (typeIndexA !== typeIndexB) {
                  return typeIndexA < typeIndexB ? -1 : 1;
               }

               return containerA.name < containerB.name ? -1 : 1;
            }
         },

         myApps: (state, getters) => {
            if (state.me === null) {
               return [];
            }

            return getters.reviewApps
               .filter(app => (app.owners ?? []).some(owner => owner.sub == state.me.sub && owner.iss == state.me.iss))
               .filter(app => app.status === "deployed");
         },

         notMyApps: (state, getters) => {
            const myAppNames = new Set(getters.myApps.map(app => app.name));

            return getters.reviewApps
               .filter(app => !myAppNames.has(app.name))
               .filter(app => app.status === "deployed");
         },

         appsWithTicket: (state, getters) => {
            return getters.notMyApps
               .filter(app => state.tickets[app.name] !== undefined);
         },

         appsWithoutTicket: (state, getters) => {
            return getters.notMyApps
               .filter(app => state.tickets[app.name] === undefined);
         },

         appBackups: (state, getters) => {
            return getters.reviewApps
               .filter(app => app.status === "backed-up");
         },

         errors: state => {
            const errors = [];

            if (state.appsError) {
               errors.push(state.appsError);
            }
            if (state.ticketsError) {
               errors.push(state.ticketsError);
            }

            return errors;
         },

         isFetchInProgress: state => state.fetchInProgress
      },
      mutations: {
         startFetch(state, { hiddenApps, appsInCreation }) {
            state.fetchInProgress = true;

            if (hiddenApps) {
               state.hiddenApps = [...state.hiddenApps, ...hiddenApps]
            }
            if (appsInCreation) {
               state.appsInCreation = { ...state.appsInCreation, ...appsInCreation };
            }
         },
         endFetch(state) {
            state.fetchInProgress = false;
         },

         storeApps(state, appsResponse) {
            if (appsResponse.type) {
               state.apps = {};
               state.appsError = appsResponse;
            }
            else {
               state.apps = appsResponse;

               // clear values from the optimistic frontend updates if the new map of apps contains
               // on does not contain anymore the app names.
               let appNames = Object.keys(state.apps);
               for (const appName of appNames) {
                  delete state.appsInCreation[appName];
               }
               state.hiddenApps = Array.from(new Set(state.hiddenApps).intersection(new Set(appNames)));

               state.appsError = null;
            }
         },

         deleteApp(state, { appName, appNameOrResponseError }) {
            if (appNameOrResponseError.type) {
               state.appsError = enrichDeletionError(appNameOrResponseError);
               state.hiddenApps = state.hiddenApps.filter(app => app != appName);
            }
            else {
               delete state.apps[appName];
               state.appsError = null;
            }
         },

         addApp(state, { appName, newAppOrResponseError  }) {
            if (newAppOrResponseError.type) {
               state.appsError = enrichDuplicationError(newAppOrResponseError);
               delete state.appsInCreation[appName];
            }
            else {
               state.apps[appName] = newAppOrResponseError;
               state.appsError = null;
            }
         },

         storeTickets(state, ticketsResponse) {
            if (ticketsResponse.type) {
               state.tickets = {};
               state.ticketsError = ticketsResponse;
            }
            else {
               state.tickets = ticketsResponse;
               state.ticketsError = null;
            }
         },

         updateServiceStatus(state, { appName, serviceName, serviceStatus }) {
            const app = state.apps[appName];
            const service = app?.services?.find(service => service.name === serviceName);

            if(!app || !service) {
               console.warn(`Could not find service "${serviceName}" of app "${appName}"`);
               return;
            }

            service.state.status = serviceStatus;
         },

         filterByAppName(state, appNameFilter) {
            state.appNameFilter = appNameFilter.toLocaleLowerCase();
            router.replace({ query: { appNameFilter } });
         }
      },
      actions: {
         fetchData(context) {
            function fetchTicketsHandler(response) {
               if (response.ok) {
                  if (response.status === 200) {
                     return response.json();
                  }
                  else {
                     return Promise.resolve({});
                  }
               }
               if (response.headers.get('Content-Type') === 'application/problem+json') {
                  return response.json();
               }
               return response.text().then(detail => ({
                  type: 'cannot-fetch-tickets',
                  title: 'Cannot fetch tickets',
                  detail
               }));
            }

            context.commit('startFetch', {});

            const appEvents = new EventSource('/api/apps', {
               fetch: (input, init) => fetch(input, {
                  ...init,
                  headers: {
                     ...init.headers,
                     Accept: 'text/vnd.prevant.v2+event-stream'
                  },
               }),
            });
            appEvents.addEventListener('message', (event) => {
               const apps = JSON.parse(event.data);

               context.commit('endFetch');
               context.commit("storeApps", apps);

               fetch('/api/apps/tickets')
                  .then(fetchTicketsHandler)
                  .then(tickets => context.commit("storeTickets", tickets));
            });
         },

         changeServiceState(context, { appName, serviceName }) {
            const app = context.state.apps[appName];
            const service = app?.services?.find(service => service.name === serviceName);

            if(!app || !service) {
               console.warn(`Could not find service "${serviceName}" of app "${appName}"`);
               return;
            }

            let newStatus;
            if (service.state.status === 'running') {
               newStatus = 'paused';
            } else {
               newStatus = 'running';
            }

            fetch(`/api/apps/${appName}/states/${serviceName}`, {
               method: 'PUT',
               headers: {
                  'Content-Type': 'application/json',
                  'Accept': 'application/json',
               },
               body: JSON.stringify({ status: newStatus })
            }).then(response => {
               if (response.status === 202) {
                  context.commit("updateServiceStatus", { appName, serviceName, serviceStatus: newStatus });
               }
            });
         },

         duplicateApp(context, { appToDuplicate, newAppName }) {
            const appsInCreation = {};
            appsInCreation[newAppName] = JSON.parse(JSON.stringify(context.state.apps[appToDuplicate]));

            context.commit('startFetch', { appsInCreation });

            fetchAndPoll(
               `/api/apps/${newAppName}?replicateFrom=${appToDuplicate}`,
               {
                  method: 'POST',
                  headers: {
                     'Content-Type': 'application/json',
                     'Accept': 'application/json'
                  },
                  body: JSON.stringify([])
               })
               .then(newAppOrResponseError => {
                  context.commit('addApp', { appName: newAppName, newAppOrResponseError });
                  context.commit('endFetch');
               });
         },

         deleteApp(context, { appName }) {
            context.commit('startFetch', { hiddenApps: [appName] });

            fetchAndPoll(
               `/api/apps/${appName}`,
               { method: 'DELETE' },
               'cannot-delete-app',
               'Cannot delete app',
            ).then(appNameOrResponseError => {
               context.commit('deleteApp', { appName, appNameOrResponseError });
               context.commit('endFetch');
            })
         }
      }
   });


   router.beforeResolve(to => {
      if (to.query.appNameFilter) {
         store.state.appNameFilter = to.query.appNameFilter;
      }
      else {
         store.state.appNameFilter = '';
      }
   });

   return store;
}

async function fetchAndPoll(url, init, problemType, problemTitle) {
   const prefer = {
      'Accept': 'application/vnd.prevant.v2+json',
      'Prefer': 'respond-async,wait=10'
   };

   async function pollLocation(location) {
      const maxRetries = 60;
      const retryInterval = 2000

      for (let attempt = 0; attempt < maxRetries; attempt++) {

         const response = await fetch(location, { headers: prefer });

         if (response.status === 202) {
            await new Promise(resolve => setTimeout(resolve, retryInterval));
         } else if (response.ok) {
            return await response.json();
         } else if (response.headers.get('Content-Type') === 'application/problem+json') {
            return await response.json();
         } else {
            return response.text().then(detail => ({
               type: problemType,
               title: problemTitle,
               detail
            }))
         }
      }

      throw new Error('Polling failed after maximum retries');
   }

   try {
      init.headers = init.headers ? { ...init.headers, ...prefer } : prefer;

      const response = await fetch(url, init);

      if (response.status == 202) {
         const location = response.headers.get('Location');
         if (!location) {
            throw new Error('Location header is missing in the 202 response');
         }

         return await pollLocation(location);
      }

      if (response.ok) {
         return await response.json();
      }

      if (response.headers.get('Content-Type') === 'application/problem+json') {
         return await response.json();
      }

      return response.text().then(detail => ({
         type: problemType,
         title: problemTitle,
         detail
      }));
   }
   catch (error) {
      console.error('Error triggering or polling API:', error);
      throw error;
   }
}

function enrichDuplicationError(error) {
  return enrichError(error, {
    403: "You need to be logged in to duplicate apps."
  });
}

function enrichDeletionError(error) {
  return enrichError(error, {
    403: "You need to be logged in to shutdown apps."
  });
}

/**
 * Takes a problem/json error object and enriches it with status code
 * depenend detail if not present already
 */
function enrichError(error, statusSpecificOverrides = {}) {
  return {
    ...error,
    detail: error.detail ?? statusSpecificOverrides[error.status] ?? "Unknown Error",
  };
}
