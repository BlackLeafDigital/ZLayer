# NatStatusResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Candidates** | [**[]NatCandidateDto**](NatCandidateDto.md) | Locally gathered ICE candidates | 
**Enabled** | **bool** | Whether NAT traversal is enabled in the daemon&#39;s config | 
**LastRefresh** | **int64** | Unix epoch seconds of the last successful STUN refresh | 
**Peers** | [**[]NatPeerDto**](NatPeerDto.md) | Per-peer NAT connectivity state | 
**RelayServerBind** | Pointer to **NullableString** | Address of the locally-bound built-in relay server, if running | [optional] 
**StunServers** | **[]string** | Configured STUN servers (host:port) | 
**TurnServers** | **[]string** | Configured TURN/relay servers (host:port) | 

## Methods

### NewNatStatusResponse

`func NewNatStatusResponse(candidates []NatCandidateDto, enabled bool, lastRefresh int64, peers []NatPeerDto, stunServers []string, turnServers []string, ) *NatStatusResponse`

NewNatStatusResponse instantiates a new NatStatusResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewNatStatusResponseWithDefaults

`func NewNatStatusResponseWithDefaults() *NatStatusResponse`

NewNatStatusResponseWithDefaults instantiates a new NatStatusResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCandidates

`func (o *NatStatusResponse) GetCandidates() []NatCandidateDto`

GetCandidates returns the Candidates field if non-nil, zero value otherwise.

### GetCandidatesOk

`func (o *NatStatusResponse) GetCandidatesOk() (*[]NatCandidateDto, bool)`

GetCandidatesOk returns a tuple with the Candidates field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCandidates

`func (o *NatStatusResponse) SetCandidates(v []NatCandidateDto)`

SetCandidates sets Candidates field to given value.


### GetEnabled

`func (o *NatStatusResponse) GetEnabled() bool`

GetEnabled returns the Enabled field if non-nil, zero value otherwise.

### GetEnabledOk

`func (o *NatStatusResponse) GetEnabledOk() (*bool, bool)`

GetEnabledOk returns a tuple with the Enabled field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetEnabled

`func (o *NatStatusResponse) SetEnabled(v bool)`

SetEnabled sets Enabled field to given value.


### GetLastRefresh

`func (o *NatStatusResponse) GetLastRefresh() int64`

GetLastRefresh returns the LastRefresh field if non-nil, zero value otherwise.

### GetLastRefreshOk

`func (o *NatStatusResponse) GetLastRefreshOk() (*int64, bool)`

GetLastRefreshOk returns a tuple with the LastRefresh field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastRefresh

`func (o *NatStatusResponse) SetLastRefresh(v int64)`

SetLastRefresh sets LastRefresh field to given value.


### GetPeers

`func (o *NatStatusResponse) GetPeers() []NatPeerDto`

GetPeers returns the Peers field if non-nil, zero value otherwise.

### GetPeersOk

`func (o *NatStatusResponse) GetPeersOk() (*[]NatPeerDto, bool)`

GetPeersOk returns a tuple with the Peers field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPeers

`func (o *NatStatusResponse) SetPeers(v []NatPeerDto)`

SetPeers sets Peers field to given value.


### GetRelayServerBind

`func (o *NatStatusResponse) GetRelayServerBind() string`

GetRelayServerBind returns the RelayServerBind field if non-nil, zero value otherwise.

### GetRelayServerBindOk

`func (o *NatStatusResponse) GetRelayServerBindOk() (*string, bool)`

GetRelayServerBindOk returns a tuple with the RelayServerBind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRelayServerBind

`func (o *NatStatusResponse) SetRelayServerBind(v string)`

SetRelayServerBind sets RelayServerBind field to given value.

### HasRelayServerBind

`func (o *NatStatusResponse) HasRelayServerBind() bool`

HasRelayServerBind returns a boolean if a field has been set.

### SetRelayServerBindNil

`func (o *NatStatusResponse) SetRelayServerBindNil(b bool)`

 SetRelayServerBindNil sets the value for RelayServerBind to be an explicit nil

### UnsetRelayServerBind
`func (o *NatStatusResponse) UnsetRelayServerBind()`

UnsetRelayServerBind ensures that no value is present for RelayServerBind, not even an explicit nil
### GetStunServers

`func (o *NatStatusResponse) GetStunServers() []string`

GetStunServers returns the StunServers field if non-nil, zero value otherwise.

### GetStunServersOk

`func (o *NatStatusResponse) GetStunServersOk() (*[]string, bool)`

GetStunServersOk returns a tuple with the StunServers field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStunServers

`func (o *NatStatusResponse) SetStunServers(v []string)`

SetStunServers sets StunServers field to given value.


### GetTurnServers

`func (o *NatStatusResponse) GetTurnServers() []string`

GetTurnServers returns the TurnServers field if non-nil, zero value otherwise.

### GetTurnServersOk

`func (o *NatStatusResponse) GetTurnServersOk() (*[]string, bool)`

GetTurnServersOk returns a tuple with the TurnServers field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTurnServers

`func (o *NatStatusResponse) SetTurnServers(v []string)`

SetTurnServers sets TurnServers field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


