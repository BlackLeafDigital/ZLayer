# PeerListResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Healthy** | **int32** | Number of healthy peers | 
**Peers** | [**[]PeerInfo**](PeerInfo.md) | List of peer information | 
**Total** | **int32** | Total number of peers | 

## Methods

### NewPeerListResponse

`func NewPeerListResponse(healthy int32, peers []PeerInfo, total int32, ) *PeerListResponse`

NewPeerListResponse instantiates a new PeerListResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewPeerListResponseWithDefaults

`func NewPeerListResponseWithDefaults() *PeerListResponse`

NewPeerListResponseWithDefaults instantiates a new PeerListResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetHealthy

`func (o *PeerListResponse) GetHealthy() int32`

GetHealthy returns the Healthy field if non-nil, zero value otherwise.

### GetHealthyOk

`func (o *PeerListResponse) GetHealthyOk() (*int32, bool)`

GetHealthyOk returns a tuple with the Healthy field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHealthy

`func (o *PeerListResponse) SetHealthy(v int32)`

SetHealthy sets Healthy field to given value.


### GetPeers

`func (o *PeerListResponse) GetPeers() []PeerInfo`

GetPeers returns the Peers field if non-nil, zero value otherwise.

### GetPeersOk

`func (o *PeerListResponse) GetPeersOk() (*[]PeerInfo, bool)`

GetPeersOk returns a tuple with the Peers field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPeers

`func (o *PeerListResponse) SetPeers(v []PeerInfo)`

SetPeers sets Peers field to given value.


### GetTotal

`func (o *PeerListResponse) GetTotal() int32`

GetTotal returns the Total field if non-nil, zero value otherwise.

### GetTotalOk

`func (o *PeerListResponse) GetTotalOk() (*int32, bool)`

GetTotalOk returns a tuple with the Total field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTotal

`func (o *PeerListResponse) SetTotal(v int32)`

SetTotal sets Total field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


