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
import { Store } from 'vuex';

const SERVICE_TYPE_ORDER = [
    'instance',
    'replica',
    'app-companion',
    'service-companion'
];

export function createStore(router) {
   const store = new Store( {
      state: {
         fetchInProgress: false,
         apps: {},
         appsError: null,
         tickets: {},
         ticketsError: null,
         appNameFilter: ''
      },
      getters: {
         appNameFilter: state => state.appNameFilter,

         reviewApps: state => {
            if ( state.apps === undefined || Object.keys( state.apps ).length == 0 ) {
               return [];
            }

            const apps = [
               appDetails( 'master' ),
               ...Object.keys( state.apps )
                  .filter( _ => _ != 'master' )
                  .map( appDetails )
                  .sort( byAppNameDesc )
            ];

            return apps
                .filter( app => app.name != null )
                .filter( app => !state.appNameFilter || app.name.toLocaleLowerCase().indexOf( state.appNameFilter.toLocaleLowerCase() ) >= 0 );

            function appDetails( name ) {
               const appContainers = state.apps[ name ];

               if (appContainers == null) {
                  return {};
               }

               const ticket = state.tickets[ name ];

               const containers = [
                  ...appContainers
                     .map( ( { name, url, openApiUrl, version, type, state } ) => {
                        return {
                            name, url, openApiUrl, version, type, status: state.status
                        };
                     } )
               ];
               containers.sort( byTypeAndName );
               return { name, ticket, containers };
            }

             function byTypeAndName(containerA, containerB) {
                 const typeIndexA = SERVICE_TYPE_ORDER.indexOf(containerA.type);
                 const typeIndexB = SERVICE_TYPE_ORDER.indexOf(containerB.type);

                 if (typeIndexA !== typeIndexB) {
                     return typeIndexA < typeIndexB ? -1 : 1;
                 }

                 return containerA.name < containerB.name ? -1 : 1;
             }

            function byAppNameDesc( appA, appB ) {
               const [ keyA, keyB ] = [ appA, appB ].map( ( { name } ) => name );
               return keyA > keyB ? -1 : 1;
            }
         },

         errors: state => {
            const errors = [];

            if( state.appsError ) {
               errors.push( state.appsError );
            }
            if( state.ticketsError ) {
               errors.push( state.ticketsError );
            }

            return errors;
         },

         isFetchInProgress: state => state.fetchInProgress
      },
      mutations: {
         startFetch( state ) {
            state.fetchInProgress = true;
         },
         endFetch( state ) {
            state.fetchInProgress = false;
         },

         storeApps( state, appsResponse ) {
            if( appsResponse.type ) {
               state.apps = {};
               state.appsError = appsResponse;
            }
            else {
               state.apps = appsResponse;
               state.appsError = null;
            }
         },

         deleteApp( state, appNameOrResponseError ) {
            if( appNameOrResponseError.type ) {
               state.appsError = appNameOrResponseError;
            }
            else {
               delete state.apps[ appNameOrResponseError ];
               state.appsError = null;
            }
         },

         addApp( state, { appName, servicesOrResponseError } ) {
            if( servicesOrResponseError.type ) {
               state.appsError = servicesOrResponseError;
            }
            else {
               state.apps[ appName ] = servicesOrResponseError;
               state.appsError = null;
            }
         },

         storeTickets( state, ticketsResponse ) {
            if( ticketsResponse.type ) {
               state.tickets = {};
               state.ticketsError = ticketsResponse;
            }
            else {
               state.tickets = ticketsResponse;
               state.ticketsError = null;
            }
         },

         updateServiceStatus( state, { appName, serviceName, serviceStatus } ) {
            const service = state.apps[appName].find(service => service.name == serviceName);
            service.state.status = serviceStatus;
         },

         filterByAppName( state, appNameFilter ) {
            state.appNameFilter = appNameFilter.toLocaleLowerCase();
            router.replace({ query: { appNameFilter } });
         }
      },
      actions: {
         fetchData( context ) {
            function fetchTicketsHandler(response) {
               if( response.ok ) {
                  if( response.status === 200 ) {
                     return response.json();
                  }
                  else {
                     return Promise.resolve({});
                  }
               }
               if( response.headers.get('Content-Type') === 'application/problem+json' ) {
                  return response.json();
               }
               return response.text().then( detail => ({
                  type: 'cannot-fetch-tickets',
                  title: 'Cannot fetch tickets',
                  detail
               }));
            }

            context.commit( 'startFetch' );

            const appEvents = new EventSource('/api/apps');
            appEvents.addEventListener('message', (event) => {
               const apps = JSON.parse(event.data);

               context.commit( 'endFetch' );
               context.commit("storeApps", apps);

               fetch( '/api/apps/tickets' )
                  .then(fetchTicketsHandler)
                  .then(tickets => context.commit("storeTickets", tickets));
            });
         },

         changeServiceState( context, { appName, serviceName } ) {
            const service = context.state.apps[ appName ].find( service => service.name === serviceName );
            let newStatus;
            if( service.state.status === 'running' ) {
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
               body: JSON.stringify( { status: newStatus } )
            }).then(response => {
               if( response.status === 202 ) {
                  context.commit( "updateServiceStatus", { appName, serviceName, serviceStatus: newStatus } );
               }
            });
         },

         duplicateApp( context, { appToDuplicate, newAppName } ) {
            context.commit( 'startFetch' );

            fetch(
               `/api/apps/${newAppName}?replicateFrom=${appToDuplicate}`,
               {
                  method: 'POST',
                  headers: {
                     'Content-Type': 'application/json',
                     'Accept': 'application/json'
                  },
                  body: JSON.stringify([])
               } )
               .then( response => {
                  const contentType = response.headers.get('Content-Type');
                  if( contentType === 'application/json' || contentType === 'application/problem+json' ) {
                     return response.json();
                  }
                  return response.text().then( detail => ({
                     type: 'cannot-duplicate-app',
                     title: 'Cannot duplicate app',
                     detail
                  }));
               })
               .then( servicesOrResponseError => {
                  context.commit( 'addApp', { appName: newAppName, servicesOrResponseError } );
                  context.commit( 'endFetch' );
               });
         },

         deleteApp( context, { appName } ) {
            context.commit( 'startFetch' );

            fetch(`/api/apps/${appName}`, { method: 'DELETE' })
               .then( response => {
                  if( response.status == 200 ) {
                     return appName;
                  }

                  if( response.headers.get('Content-Type') === 'application/problem+json' ) {
                     return response.json();
                  }
                  return response.text().then( detail => ({
                     type: 'cannot-delete-app',
                     title: 'Cannot delete app',
                     detail
                  }));
               })
               .then( appNameOrResponseError => {
                  context.commit( 'deleteApp', appNameOrResponseError );
                  context.commit( 'endFetch' );
               })
         }
      }
   } );


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
