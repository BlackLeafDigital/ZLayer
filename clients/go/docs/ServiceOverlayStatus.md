# ServiceOverlayStatus

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**BridgesByNode** | [**map[string]BridgeInfo**](BridgeInfo.md) | One entry per node where this service has at least one container attached. Keys are node IDs as stringified. | 
**Mode** | [**OverlayMode**](OverlayMode.md) | Mode the daemon resolved for this service (after &#x60;resolve_v0_51&#x60;). | 
**Service** | **string** |  | 

## Methods

### NewServiceOverlayStatus

`func NewServiceOverlayStatus(bridgesByNode map[string]BridgeInfo, mode OverlayMode, service string, ) *ServiceOverlayStatus`

NewServiceOverlayStatus instantiates a new ServiceOverlayStatus object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewServiceOverlayStatusWithDefaults

`func NewServiceOverlayStatusWithDefaults() *ServiceOverlayStatus`

NewServiceOverlayStatusWithDefaults instantiates a new ServiceOverlayStatus object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetBridgesByNode

`func (o *ServiceOverlayStatus) GetBridgesByNode() map[string]BridgeInfo`

GetBridgesByNode returns the BridgesByNode field if non-nil, zero value otherwise.

### GetBridgesByNodeOk

`func (o *ServiceOverlayStatus) GetBridgesByNodeOk() (*map[string]BridgeInfo, bool)`

GetBridgesByNodeOk returns a tuple with the BridgesByNode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBridgesByNode

`func (o *ServiceOverlayStatus) SetBridgesByNode(v map[string]BridgeInfo)`

SetBridgesByNode sets BridgesByNode field to given value.


### GetMode

`func (o *ServiceOverlayStatus) GetMode() OverlayMode`

GetMode returns the Mode field if non-nil, zero value otherwise.

### GetModeOk

`func (o *ServiceOverlayStatus) GetModeOk() (*OverlayMode, bool)`

GetModeOk returns a tuple with the Mode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMode

`func (o *ServiceOverlayStatus) SetMode(v OverlayMode)`

SetMode sets Mode field to given value.


### GetService

`func (o *ServiceOverlayStatus) GetService() string`

GetService returns the Service field if non-nil, zero value otherwise.

### GetServiceOk

`func (o *ServiceOverlayStatus) GetServiceOk() (*string, bool)`

GetServiceOk returns a tuple with the Service field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetService

`func (o *ServiceOverlayStatus) SetService(v string)`

SetService sets Service field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


